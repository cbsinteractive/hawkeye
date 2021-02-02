use color_eyre::eyre::WrapErr;
use color_eyre::Result;
use image::imageops::FilterType;
use image::ImageFormat;
use log::debug;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::process;
use std::time::Duration;
use std::path::Path;

pub const SLATE_SIZE: (u32, u32) = (213, 120);

pub fn load_img(url: &str) -> Result<Box<dyn Read>> {
    let ext = Path::new(url).extension().ok_or_else(|| {
        color_eyre::eyre::eyre!("Slate contains no file extension")
    })?.to_str().unwrap();

    debug!("Slate file extension: {}", ext);

    let path = if url.starts_with("http://") || url.starts_with("https://") {
        debug!("Downloading from url: {}", url);
        let res = ureq::get(url)
            .timeout(Duration::from_secs(10))
            .timeout_connect(1000)
            .call();
        if res.error() {
            return Err(color_eyre::eyre::eyre!(
                "HTTP error ({}) while calling URL of backend: {}",
                res.status(),
                url
            ));
        }

        let temp_path = format!("/tmp/hawkeye_downloaded_{}.{}", process::id(), ext);
        let mut outfile =
            File::create(temp_path.as_str()).wrap_err("Could not create temp file for download")?;

        let mut buffer = Vec::with_capacity(1024);
        let mut reader = res.into_reader();
        // TODO: Maybe there is a better way to do this?
        loop {
            let p = reader.read_to_end(&mut buffer)?;
            outfile.write(buffer.as_slice())?;
            buffer.clear();
            if p == 0 {
                break;
            }
        }

        temp_path
    } else {
        url.replace("file://", "")
    };

    // TODO: if video:
    //      extract_video_frame return a buffer already in 213x120

    debug!("Loading slate image from file");
    let img = image::open(path)
        .wrap_err("Could not open image")?
        .resize_exact(SLATE_SIZE.0, SLATE_SIZE.1, FilterType::Triangle);
    let mut contents = Vec::new();
    img.write_to(&mut contents, ImageFormat::Png)
        .wrap_err("Could not write to temp file")?;
    Ok(Box::new(Cursor::new(contents)))
}
