//! Screenshot pixels: resizing to the model's size, the cursor marker, PNG encoding.

use image::{ImageBuffer, Rgba, RgbaImage, imageops::FilterType};

/// Resizes RGBA pixels to exactly `to`. Bicubic (Catmull-Rom, a = -0.5), the same kernel
/// as the Pillow-compatible bicubic llama.cpp uses for Qwen-family projectors, with the
/// support widened when shrinking so small text does not alias away.
pub fn resize(rgba: Vec<u8>, from: (u32, u32), to: (u32, u32)) -> Result<RgbaImage, String> {
    let img: RgbaImage = ImageBuffer::from_raw(from.0, from.1, rgba)
        .ok_or_else(|| "the capture has the wrong number of bytes for its size".to_string())?;
    if from == to {
        return Ok(img);
    }
    if to.0 == 0 || to.1 == 0 {
        return Err("the target image size is empty".into());
    }
    Ok(image::imageops::resize(
        &img,
        to.0,
        to.1,
        FilterType::CatmullRom,
    ))
}

/// The outline of the cursor marker: a classic arrow pointer, tip at (0, 0), 12 × 19 at
/// size 1.
const ARROW: [(f64, f64); 7] = [
    (0.0, 0.0),
    (0.0, 16.0),
    (4.0, 12.5),
    (7.0, 19.0),
    (9.5, 18.0),
    (6.5, 11.5),
    (12.0, 11.5),
];

fn inside(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut c = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

/// Draws an arrow pointer with its tip at (`x`, `y`) in image pixels: white with a black
/// outline, so it shows on any background. `scale` 1 is 12 × 19 pixels.
pub fn draw_cursor(img: &mut RgbaImage, x: f64, y: f64, scale: f64) {
    let scale = scale.clamp(0.5, 4.0);
    let outline = 1.5;
    let (w, h) = (img.width() as i64, img.height() as i64);
    let x0 = (x - outline).floor() as i64;
    let y0 = (y - outline).floor() as i64;
    let x1 = (x + 12.0 * scale + outline).ceil() as i64;
    let y1 = (y + 19.0 * scale + outline).ceil() as i64;
    let scaled: Vec<(f64, f64)> = ARROW
        .iter()
        .map(|(ax, ay)| (x + ax * scale, y + ay * scale))
        .collect();
    for py in y0.max(0)..y1.min(h) {
        for px in x0.max(0)..x1.min(w) {
            let (cx, cy) = (px as f64 + 0.5, py as f64 + 0.5);
            let fill = inside(&scaled, cx, cy);
            let edge = !fill
                && [
                    (-outline, 0.0),
                    (outline, 0.0),
                    (0.0, -outline),
                    (0.0, outline),
                    (-outline, -outline),
                    (outline, outline),
                    (-outline, outline),
                    (outline, -outline),
                ]
                .iter()
                .any(|(dx, dy)| inside(&scaled, cx + dx, cy + dy));
            if fill {
                img.put_pixel(px as u32, py as u32, Rgba([255, 255, 255, 255]));
            } else if edge {
                img.put_pixel(px as u32, py as u32, Rgba([0, 0, 0, 255]));
            }
        }
    }
}

/// PNG bytes of the image, without the alpha channel (screens are opaque).
pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut rgb = Vec::with_capacity((img.width() * img.height() * 3) as usize);
    for px in img.pixels() {
        rgb.extend_from_slice(&px.0[..3]);
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            &rgb,
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("the screenshot could not be encoded: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&c);
        }
        v
    }

    #[test]
    fn resize_produces_exactly_the_requested_size() {
        let img = resize(solid(300, 200, [10, 200, 30, 255]), (300, 200), (112, 84)).unwrap();
        assert_eq!(img.dimensions(), (112, 84));
        // A flat colour stays that colour.
        assert_eq!(img.get_pixel(50, 40).0, [10, 200, 30, 255]);
        // Same size is returned unchanged.
        let same = resize(solid(4, 4, [1, 2, 3, 255]), (4, 4), (4, 4)).unwrap();
        assert_eq!(same.dimensions(), (4, 4));
        // Wrong byte count is an error, not a panic.
        assert!(resize(vec![0; 10], (4, 4), (2, 2)).is_err());
        assert!(resize(solid(4, 4, [0; 4]), (4, 4), (0, 2)).is_err());
    }

    #[test]
    fn cursor_marker_has_its_tip_at_the_point_and_an_outline() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(60, 60, Rgba([128, 128, 128, 255]));
        draw_cursor(&mut img, 20.0, 20.0, 1.0);
        // Inside the arrow, just below the tip: white.
        assert_eq!(img.get_pixel(21, 24).0, [255, 255, 255, 255]);
        // Left of the arrow's left edge: black outline.
        assert_eq!(img.get_pixel(19, 26).0, [0, 0, 0, 255]);
        // Far away: untouched.
        assert_eq!(img.get_pixel(50, 5).0, [128, 128, 128, 255]);
        // Near the edges it is clipped, not a panic.
        draw_cursor(&mut img, 58.0, 58.0, 2.0);
        draw_cursor(&mut img, -5.0, -5.0, 1.0);
    }

    #[test]
    fn png_round_trips() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(8, 6, Rgba([1, 2, 3, 255]));
        img.put_pixel(7, 5, Rgba([200, 100, 50, 255]));
        let png = encode_png(&img).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let back = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(back.dimensions(), (8, 6));
        assert_eq!(back.get_pixel(7, 5).0, [200, 100, 50]);
        assert_eq!(back.get_pixel(0, 0).0, [1, 2, 3]);
    }
}
