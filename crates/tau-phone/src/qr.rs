//! Reads a pairing code from the camera's frames.
//!
//! The viewfinder hands over each frame's luminance, one frame at a
//! time, until one holds the code the computer shows.

use tau_remote::PairingCode;

/// Frames are subsampled to at most this many pixels on their long side
/// first: a code on a screen reads as well, and a large frame takes too
/// long to search.
const LONG_SIDE: usize = 1280;

/// A camera frame's luminance: a byte a pixel, `stride` bytes a row.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    luma: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> Frame<'a> {
    /// None if `luma` is too short for the frame it should hold.
    pub fn new(
        luma: &'a [u8],
        width: usize,
        height: usize,
        stride: usize,
    ) -> Option<Self> {
        let needed = stride.checked_mul(height.checked_sub(1)?)?;
        let fits = width > 0
            && stride >= width
            && luma.len() >= needed.checked_add(width)?;
        fits.then_some(Self {
            luma,
            width,
            height,
            stride,
        })
    }
}

/// The text of the tau pairing code in `frame`. None while there is
/// none, and while the only codes in view are some other QR codes.
///
/// rqrr's own thresholding misses some turned codes whose edges the
/// lens blurred; the frame cut to black and white at its mean
/// brightness reads them, so a frame that reads nothing is tried again
/// that way.
pub fn pairing_code(frame: Frame<'_>) -> Option<String> {
    let step = frame.width.max(frame.height).div_ceil(LONG_SIDE);
    let (width, height) = (frame.width / step, frame.height / step);
    let pixel =
        |x: usize, y: usize| frame.luma[y * step * frame.stride + x * step];
    read(width, height, pixel).or_else(|| {
        let total: u64 = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| u64::from(pixel(x, y)))
            .sum();
        let mean = total / (width * height).max(1) as u64;
        read(width, height, |x, y| {
            if u64::from(pixel(x, y)) < mean {
                0
            } else {
                255
            }
        })
    })
}

/// The pairing code among the QR codes rqrr finds in a `width` by
/// `height` image, `pixel` giving each one's luminance.
fn read(
    width: usize,
    height: usize,
    pixel: impl Fn(usize, usize) -> u8,
) -> Option<String> {
    rqrr::PreparedImage::prepare_from_greyscale(width, height, pixel)
        .detect_grids()
        .into_iter()
        .filter_map(|grid| grid.decode().ok().map(|(_, text)| text))
        .find(|text| text.parse::<PairingCode>().is_ok())
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    const CODE: &str = "tau-pair://100.84.12.7:7443?host=cfcosta-desk&fp=4f92d5185b9ee12467aaed3073b6f93c7fc205488bce115497da1d60a3e6296c&code=K7QM-2XPA";

    /// A camera frame: its luminance and how it is laid out.
    struct Shot {
        luma: Vec<u8>,
        width: usize,
        height: usize,
        stride: usize,
    }

    impl Shot {
        fn frame(&self) -> Frame<'_> {
            Frame::new(&self.luma, self.width, self.height, self.stride)
                .unwrap()
        }
    }

    /// How a code sits in a shot.
    struct Pose {
        /// Pixels a module.
        scale: f64,
        /// Turned this many degrees about the frame's center.
        degrees: f64,
        width: usize,
        height: usize,
        /// Bytes at the end of each row that are not pixels.
        padding: usize,
    }

    /// `text` as a QR code shown on a screen, in a camera frame: dark
    /// modules on white, the rest of the frame grey.
    fn shoot(text: &str, pose: &Pose) -> Shot {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let modules = code.width() as isize;
        let colors = code.to_colors();
        let stride = pose.width + pose.padding;
        let (sin, cos) = pose.degrees.to_radians().sin_cos();
        let (cx, cy) = (pose.width as f64 / 2., pose.height as f64 / 2.);
        let half = modules as f64 / 2.;
        // What the code shows at a point of the frame, if it is there:
        // dark modules on white, with four white modules around.
        let shade = |x: f64, y: f64| {
            // Back from the frame to the code, in modules from its
            // corner.
            let (dx, dy) = (x - cx, y - cy);
            let mx = ((dx * cos + dy * sin) / pose.scale + half).floor();
            let my = ((dy * cos - dx * sin) / pose.scale + half).floor();
            let (mx, my) = (mx as isize, my as isize);
            let near = -4..modules + 4;
            if !near.contains(&mx) || !near.contains(&my) {
                return None;
            }
            let inside =
                (0..modules).contains(&mx) && (0..modules).contains(&my);
            let dark = inside
                && colors[(my * modules + mx) as usize] == qrcode::Color::Dark;
            Some(if dark { 0x10 } else { 0xf0 })
        };
        let mut luma = vec![0; stride * pose.height];
        for y in 0..pose.height {
            for x in 0..pose.width {
                // A lens blurs: each pixel is the mean of what falls in
                // it, sampled four by four; the frame around is grey.
                let mut sum = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let (px, py) = (
                            x as f64 + (sx as f64 + 0.5) / 4.,
                            y as f64 + (sy as f64 + 0.5) / 4.,
                        );
                        sum += shade(px, py).unwrap_or(0x5a);
                    }
                }
                luma[y * stride + x] = (sum / 16) as u8;
            }
        }
        // What is past the pixels is not part of the picture.
        for row in luma.chunks_mut(stride) {
            row[pose.width..].fill(0);
        }
        Shot {
            luma,
            width: pose.width,
            height: pose.height,
            stride,
        }
    }

    fn upright(width: usize, height: usize, scale: f64) -> Pose {
        Pose {
            scale,
            degrees: 0.,
            width,
            height,
            padding: 0,
        }
    }

    #[test]
    fn reads_a_pairing_code() {
        let shot = shoot(CODE, &upright(640, 480, 6.));
        assert_eq!(pairing_code(shot.frame()), Some(CODE.to_owned()));
    }

    /// A code rqrr alone missed: seven pixels a module, turned twelve
    /// degrees, with blurred edges. Its fallback reads it.
    #[test]
    fn a_blurred_turned_code_reads() {
        let pose = Pose {
            degrees: 12.0625,
            ..upright(641, 641, 7.)
        };
        let shot = shoot(CODE, &pose);
        assert_eq!(pairing_code(shot.frame()), Some(CODE.to_owned()));
    }

    #[test]
    fn a_large_frame_is_scaled_down_and_still_reads() {
        let shot = shoot(CODE, &upright(2400, 2000, 32.));
        assert_eq!(pairing_code(shot.frame()), Some(CODE.to_owned()));
    }

    #[test]
    fn a_frame_without_a_code_reads_nothing() {
        let blank = Shot {
            luma: vec![0xf0; 320 * 240],
            width: 320,
            height: 240,
            stride: 320,
        };
        assert_eq!(pairing_code(blank.frame()), None);
    }

    #[test]
    fn other_qr_codes_are_passed_over() {
        let pose = upright(480, 480, 6.);
        for text in [
            "https://example.com/tau",
            // A tau code missing its secret.
            "tau-pair://100.84.12.7:7443?host=cfcosta-desk&fp=4f92d5185b9ee12467aaed3073b6f93c7fc205488bce115497da1d60a3e6296c",
            "tau-pair://desk:1?host=d&fp=4fa1&code=K7QM2XPA",
        ] {
            assert_eq!(
                pairing_code(shoot(text, &pose).frame()),
                None,
                "{text}"
            );
        }
    }

    #[test]
    fn a_short_buffer_is_not_a_frame() {
        assert!(Frame::new(&[0; 99], 10, 10, 10).is_none());
        assert!(Frame::new(&[0; 100], 10, 10, 10).is_some());
        // The last row needs no padding after it.
        assert!(Frame::new(&[0; 118], 10, 10, 12).is_some());
        assert!(Frame::new(&[0; 100], 10, 10, 9).is_none());
        assert!(Frame::new(&[], 0, 0, 0).is_none());
    }

    /// Turned any way, a code reads from six pixels a module: what the
    /// viewfinder's square holds at the frame's 720 rows. Smaller and
    /// turned, rqrr misses some.
    #[hegel::test(test_cases = 40)]
    fn a_code_reads_turned_and_at_any_size(tc: TestCase) {
        let pose = Pose {
            scale: tc.draw(gs::floats::<f64>().min_value(6.).max_value(10.)),
            degrees: tc.draw(gs::floats::<f64>().min_value(0.).max_value(360.)),
            width: tc
                .draw(gs::integers::<usize>().min_value(640).max_value(900)),
            height: tc
                .draw(gs::integers::<usize>().min_value(640).max_value(900)),
            padding: tc.draw(gs::integers::<usize>().max_value(64)),
        };
        let shot = shoot(CODE, &pose);
        assert_eq!(pairing_code(shot.frame()), Some(CODE.to_owned()));
    }

    #[hegel::test(test_cases = 40)]
    fn noise_reads_nothing(tc: TestCase) {
        let width = tc.draw(gs::integers::<usize>().min_value(1).max_value(96));
        let height =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(96));
        let stride = width + tc.draw(gs::integers::<usize>().max_value(16));
        let luma: Vec<u8> = tc.draw(
            gs::vecs(gs::integers::<u8>())
                .min_size(stride * height)
                .max_size(stride * height),
        );
        let frame = Frame::new(&luma, width, height, stride).unwrap();
        assert_eq!(pairing_code(frame), None);
    }
}
