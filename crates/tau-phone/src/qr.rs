//! Reads a pairing code from a photo of the computer's screen.

use std::path::Path;

use image::{GrayImage, imageops::FilterType};

/// Photos are downscaled to this width first: a code on a screen reads
/// as well, and a full photo takes seconds to search.
const WIDTH: u32 = 1280;

/// The text of the QR code in the photo at `path`, in words if none
/// reads.
pub fn read(path: &Path) -> Result<String, String> {
    let photo = image::open(path)
        .map_err(|error| format!("tau could not open the photo: {error}"))?;
    decode(photo.to_luma8())
}

pub fn decode(photo: GrayImage) -> Result<String, String> {
    let photo = if photo.width() > WIDTH {
        let height = photo.height() * WIDTH / photo.width();
        image::imageops::resize(&photo, WIDTH, height, FilterType::Triangle)
    } else {
        photo
    };
    let mut prepared = rqrr::PreparedImage::prepare(photo);
    prepared
        .detect_grids()
        .into_iter()
        .find_map(|grid| grid.decode().ok().map(|(_, text)| text))
        .ok_or_else(|| {
            "No pairing code in the photo. Fill the frame with the code on \
             your computer and try again."
                .into()
        })
}

#[cfg(test)]
mod tests {
    use image::Luma;

    use super::*;

    /// `text` as a QR code the way a screen shows it: `scale` pixels a
    /// module, a white margin around.
    fn photo(text: &str, scale: u32) -> GrayImage {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let width = code.width() as u32;
        let colors = code.to_colors();
        let side = (width + 8) * scale;
        GrayImage::from_fn(side, side, |x, y| {
            let (mx, my) = ((x / scale).wrapping_sub(4), (y / scale).wrapping_sub(4));
            let dark = mx < width
                && my < width
                && colors[(my * width + mx) as usize] == qrcode::Color::Dark;
            Luma([if dark { 0 } else { 255 }])
        })
    }

    #[test]
    fn reads_a_pairing_code() {
        let text = "tau-pair://100.84.12.7:7443?host=cfcosta-desk&fp=4f92d5185b9ee12467aaed3073b6f93c7fc205488bce115497da1d60a3e6296c&code=K7QM-2XPA";
        assert_eq!(decode(photo(text, 6)), Ok(text.to_owned()));
        // A large photo is scaled down first, and still reads.
        assert_eq!(decode(photo(text, 40)), Ok(text.to_owned()));
    }

    #[test]
    fn a_photo_without_a_code_says_so() {
        let blank = GrayImage::from_pixel(200, 200, Luma([255]));
        assert!(decode(blank).unwrap_err().starts_with("No pairing code"));
    }
}
