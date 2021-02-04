use color_eyre::Result;
use dssim::{DssimImage, ToRGBAPLU, RGBAPLU};
use imgref::{Img, ImgVec};
use load_image::{Image, ImageData};

pub struct SlateDetector {
    slate: DssimImage<f32>,
    similarity_algorithm: dssim::Dssim,
}

impl SlateDetector {
    pub fn new(slate: &[u8]) -> Result<Self> {
        let similarity_algorithm = dssim::Dssim::new();
        let slate_img = load_data(slate)?;
        let slate = similarity_algorithm.create_image(&slate_img).unwrap();

        Ok(Self {
            slate,
            similarity_algorithm,
        })
    }

    pub fn is_match(&self, image_buffer: &[u8]) -> bool {
        let frame_img = load_data(image_buffer).unwrap();
        let frame = self.similarity_algorithm.create_image(&frame_img).unwrap();

        let (res, _) = self.similarity_algorithm.compare(&self.slate, frame);
        let val: f64 = res.into();
        let val = (val * 1000f64) as u32;

        val <= 900u32
    }
}

fn load_data(data: &[u8]) -> Result<ImgVec<RGBAPLU>> {
    let img = load_image::load_image_data(data, false)?;
    Ok(match_img_bitmap(img))
}

fn match_img_bitmap(img: Image) -> ImgVec<RGBAPLU> {
    match img.bitmap {
        ImageData::RGB8(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::RGB16(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::RGBA8(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::RGBA16(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::GRAY8(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::GRAY16(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::GRAYA8(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
        ImageData::GRAYA16(ref bitmap) => Img::new(bitmap.to_rgbaplu(), img.width, img.height),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;
    use std::io::Read;
    use std::path::Path;

    fn read_bytes<P: AsRef<Path>>(path: P) -> Vec<u8> {
        let mut slate_img =
            std::fs::File::open(path).expect("We must have this image in the /resources folder");
        let mut buffer = Vec::new();
        slate_img.read_to_end(&mut buffer).unwrap();
        buffer
    }

    #[test]
    fn compare_equal_images() {
        let mut slate =
            File::open("../resources/slate_120px.jpg").expect("Missing file in resources folder");
        let mut buffer = Vec::new();
        slate
            .read_to_end(&mut buffer)
            .expect("Failed to write to buffer");
        let detector = SlateDetector::new(buffer.as_slice()).unwrap();
        let slate_img = read_bytes("../resources/slate_120px.jpg");

        assert!(detector.is_match(slate_img.as_slice()));
    }

    #[test]
    fn compare_diff_images() {
        let mut slate =
            File::open("../resources/slate_120px.jpg").expect("Missing file in resources folder");
        let mut buffer = Vec::new();
        slate
            .read_to_end(&mut buffer)
            .expect("Failed to write to buffer");
        let detector = SlateDetector::new(buffer.as_slice()).unwrap();
        let frame_img = read_bytes("../resources/non-slate_120px.jpg");

        assert_eq!(detector.is_match(frame_img.as_slice()), false);
    }
}
