use crate::video_stream::VideoStream;
use color_eyre::eyre::WrapErr;
use color_eyre::Result;
use image::imageops::FilterType;
use image::ImageFormat;
use log::debug;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use std::convert::{TryFrom, TryInto};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const SLATE_SIZE: (u32, u32) = (213, 120);
const MEGABYTES: usize = 1024 * 1024;
const VIDEO_FILE_EXTENSIONS: [&str; 2] = ["mp4", "mkv"];

pub fn load_img(url: &str) -> Result<Vec<u8>> {
    let temp_file: TempFile = Url::new(url).try_into()?;

    let contents = if temp_file.is_video() {
        let mut pipeline = FrameCapture::new(temp_file, SLATE_SIZE);
        pipeline.get_first_frame_contents()?
    } else {
        let path = temp_file.full_path();
        debug!("Loading slate image from file: {}", path);
        let img = image::open(path.as_str())
            .wrap_err("Failed to open image")?
            .resize_exact(SLATE_SIZE.0, SLATE_SIZE.1, FilterType::Triangle);
        let mut contents = Vec::new();
        img.write_to(&mut contents, ImageFormat::Png)
            .wrap_err("Failed to write to temp file")?;
        contents
    };

    if log::max_level() <= log::Level::Debug {
        let mut f = TempFile::new("debug", "png")?;
        f.write_all(contents.as_slice())?;
        debug!("Wrote to debug file: {}", f.full_path())
    }

    Ok(contents)
}

pub trait FileLike {
    fn full_path(&self) -> String;

    fn extension(&self) -> Result<String> {
        Ok(String::from(
            Path::new(self.full_path().as_str())
                .extension()
                .ok_or_else(|| color_eyre::eyre::eyre!("File does not have extension"))?
                .to_str()
                .unwrap(),
        ))
    }
}

pub struct Url {
    url: String,
}

impl Url {
    fn new<S: AsRef<str>>(url: S) -> Self {
        Self {
            url: String::from(url.as_ref()),
        }
    }

    fn is_http(&self) -> bool {
        self.url.starts_with("http://") || self.url.starts_with("https://")
    }
}

impl FileLike for Url {
    fn full_path(&self) -> String {
        self.url.clone()
    }
}

pub struct TempFile {
    file: File,
    path: String,
}

impl TempFile {
    pub fn new<S: AsRef<str>, T: AsRef<str>>(name: S, ext: T) -> Result<Self> {
        let path = Self::file_path(name.as_ref(), ext.as_ref());
        Ok(Self {
            file: File::create(path.as_str()).wrap_err("Failed to create temp file")?,
            path,
        })
    }

    pub fn from_original<S: AsRef<str>>(full_path: S) -> Result<Self> {
        Ok(Self {
            file: File::open(full_path.as_ref()).wrap_err("Failed open provided path")?,
            path: String::from(full_path.as_ref()),
        })
    }

    fn is_video(&self) -> bool {
        VIDEO_FILE_EXTENSIONS
            .iter()
            .any(|v| *v == self.extension().unwrap_or_else(|_| String::new()))
    }

    fn write_all<R: Read>(&mut self, mut reader: R) -> Result<()> {
        let mut buffer = Vec::with_capacity(5 * MEGABYTES);
        loop {
            let p = reader.read_to_end(&mut buffer)?;
            self.file.write_all(buffer.as_slice())?;
            buffer.clear();
            if p == 0 {
                break;
            }
        }
        Ok(())
    }

    fn file_path(name: &str, ext: &str) -> String {
        let rand_string: String = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(10)
            .map(char::from)
            .collect();
        format!("/tmp/hwk_{}_{}.{}", rand_string, name, ext)
    }
}

impl FileLike for TempFile {
    fn full_path(&self) -> String {
        self.path.clone()
    }
}

impl TryFrom<Url> for TempFile {
    type Error = color_eyre::eyre::Report;

    fn try_from(url: Url) -> Result<Self, Self::Error> {
        let f = if url.is_http() {
            let path = url.full_path();
            debug!("Downloading slate from: {}", path);
            let res = ureq::get(path.as_str())
                .timeout(Duration::from_secs(10))
                .timeout_connect(1000)
                .call();
            if res.error() {
                return Err(color_eyre::eyre::eyre!(
                    "HTTP error ({}) while calling URL of backend: {}",
                    res.status(),
                    url.full_path()
                ));
            }
            let mut temp_file = TempFile::new("downloaded", url.extension()?)?;
            temp_file.write_all(res.into_reader())?;
            temp_file
        } else {
            TempFile::from_original(url.full_path().replace("file://", "").as_str())?
        };

        Ok(f)
    }
}

pub struct FrameCapture {
    source: TempFile,
    frame_size: (u32, u32),
}

impl FrameCapture {
    pub fn new(source: TempFile, frame_size: (u32, u32)) -> Self {
        Self { source, frame_size }
    }

    pub fn get_first_frame_contents(&mut self) -> Result<Vec<u8>> {
        let pipeline = format!(
            "uridecodebin uri=file://{} ! videoconvert ! videoscale ! capsfilter caps=\"video/x-raw, width={}, height={}\"",
            self.source.full_path(),
            self.frame_size.0,
            self.frame_size.1
        );
        for frame in VideoStream::new(pipeline) {
            match frame? {
                Some(contents) => return Ok(contents),
                None => continue,
            }
        }
        Err(color_eyre::eyre::eyre!("Failed to capture video frame"))
    }
}
