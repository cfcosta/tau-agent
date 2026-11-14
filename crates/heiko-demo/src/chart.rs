//! The demo's chart: a PNG drawn here, for the canvas and the uploads
//! screens.

/// A line chart of two falling series on a dark ground, as a PNG.
pub fn chart(width: u32, height: u32) -> Vec<u8> {
    use image::{Rgb, RgbImage};
    let mut pixels = RgbImage::from_pixel(width, height, Rgb([22, 24, 29]));
    let (left, right, top, bottom) = (80, width - 40, 40, height - 60);
    let mut line = |from: (f32, f32), to: (f32, f32), color, thick: i32| {
        let steps =
            ((to.0 - from.0).abs().max((to.1 - from.1).abs()) as i32).max(1);
        for step in 0..=steps {
            let k = step as f32 / steps as f32;
            let x = from.0 + (to.0 - from.0) * k;
            let y = from.1 + (to.1 - from.1) * k;
            for dx in -thick..=thick {
                for dy in -thick..=thick {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if px >= 0
                        && py >= 0
                        && (px as u32) < width
                        && (py as u32) < height
                    {
                        pixels.put_pixel(px as u32, py as u32, color);
                    }
                }
            }
        }
    };
    let grid = Rgb([44, 48, 58]);
    for n in 0..=5 {
        let y = top as f32 + (bottom - top) as f32 * n as f32 / 5.;
        line((left as f32, y), (right as f32, y), grid, 0);
    }
    for n in 0..=8 {
        let x = left as f32 + (right - left) as f32 * n as f32 / 8.;
        line((x, top as f32), (x, bottom as f32), grid, 0);
    }
    let axis = Rgb([120, 126, 140]);
    line(
        (left as f32, top as f32),
        (left as f32, bottom as f32),
        axis,
        1,
    );
    line(
        (left as f32, bottom as f32),
        (right as f32, bottom as f32),
        axis,
        1,
    );
    let series = |scale: f32, floor: f32, wobble: f32| {
        (0..=40)
            .map(|n| {
                let t = n as f32 / 40.;
                let value =
                    floor + scale * (-3. * t).exp() + wobble * (t * 23.).sin();
                let x = left as f32 + (right - left) as f32 * t;
                let y = bottom as f32 - (bottom - top) as f32 * value;
                (x, y)
            })
            .collect::<Vec<_>>()
    };
    for (points, color) in [
        (series(0.78, 0.16, 0.012), Rgb([240, 150, 90])),
        (series(0.5, 0.08, 0.008), Rgb([110, 170, 250])),
    ] {
        for pair in points.windows(2) {
            line(pair[0], pair[1], color, 2);
        }
    }
    let mut bytes = Vec::new();
    pixels
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("a PNG encodes");
    bytes
}
