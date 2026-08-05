//! アバター未設定のローカル actor 向け決定論的アイコン。

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn seeded(actor_id: i64) -> u64 {
    actor_id
        .to_le_bytes()
        .into_iter()
        .fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
        })
}

/// actor ID から常に同じ代替アバター URL を組み立てる。
pub fn fallback_avatar_url(local_domain: &str, actor_id: i64) -> String {
    format!("https://{local_domain}/api/avatars/{actor_id}?v=5")
}

fn hsl_to_rgb(hue: u64, saturation: f32, lightness: f32) -> Rgb<u8> {
    let h = hue as f32 / 360.0;
    let a = saturation * lightness.min(1.0 - lightness);
    let channel = |offset: f32| {
        let k = (offset + h * 12.0) % 12.0;
        let value = lightness - a * (-1.0_f32).max((k - 3.0).min(9.0 - k).min(1.0));
        (value * 255.0).round() as u8
    };
    Rgb([channel(0.0), channel(8.0), channel(4.0)])
}

fn paint_disc(image: &mut RgbImage, cx: f32, cy: f32, radius: f32, color: Rgb<u8>) {
    let radius_sq = radius * radius;
    let min_x = (cx - radius).floor().max(0.0) as u32;
    let max_x = (cx + radius).ceil().min(image.width() as f32 - 1.0) as u32;
    let min_y = (cy - radius).floor().max(0.0) as u32;
    let max_y = (cy + radius).ceil().min(image.height() as f32 - 1.0) as u32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= radius_sq {
                *image.get_pixel_mut(x, y) = color;
            }
        }
    }
}

fn paint_line(image: &mut RgbImage, from: (f32, f32), to: (f32, f32), width: f32) {
    let (vx, vy) = (to.0 - from.0, to.1 - from.1);
    let length_sq = vx * vx + vy * vy;
    let radius = width / 2.0;
    let min_x = (from.0.min(to.0) - radius).floor().max(0.0) as u32;
    let max_x = (from.0.max(to.0) + radius)
        .ceil()
        .min(image.width() as f32 - 1.0) as u32;
    let min_y = (from.1.min(to.1) - radius).floor().max(0.0) as u32;
    let max_y = (from.1.max(to.1) + radius)
        .ceil()
        .min(image.height() as f32 - 1.0) as u32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let point = (x as f32 + 0.5, y as f32 + 0.5);
            let t = if length_sq == 0.0 {
                0.0
            } else {
                (((point.0 - from.0) * vx + (point.1 - from.1) * vy) / length_sq).clamp(0.0, 1.0)
            };
            let dx = point.0 - (from.0 + t * vx);
            let dy = point.1 - (from.1 + t * vy);
            if dx * dx + dy * dy <= width * width / 4.0 {
                *image.get_pixel_mut(x, y) = Rgb([0, 0, 0]);
            }
        }
    }
}

fn paint_curve(
    image: &mut RgbImage,
    start: (f32, f32),
    control: (f32, f32),
    end: (f32, f32),
    width: f32,
) {
    let mut previous = start;
    for step in 1..=32 {
        let t = step as f32 / 32.0;
        let one_minus_t = 1.0 - t;
        let current = (
            one_minus_t * one_minus_t * start.0 + 2.0 * one_minus_t * t * control.0 + t * t * end.0,
            one_minus_t * one_minus_t * start.1 + 2.0 * one_minus_t * t * control.1 + t * t * end.1,
        );
        paint_line(image, previous, current, width);
        previous = current;
    }
}

/// SVG非対応のMisskeyクライアントでも表示できる決定論的PNGを生成する。
pub fn fallback_avatar_png(actor_id: i64) -> Vec<u8> {
    const SCALE: f32 = 4.0;
    let seed = seeded(actor_id);
    let hue = seed % 360;
    let background_hue = (hue + 90) % 360;
    let horizontal = ((seed >> 9) % 3) as i32 - 1;
    let eye_y = 43 + (((seed >> 17) % 3) as i32 - 1) * 5;
    let eye_gap = 18 + ((seed >> 25) % 3) as i32 * 5;
    let center_x = 50 + horizontal * 9;

    let mut image = RgbImage::from_pixel(400, 400, hsl_to_rgb(background_hue, 0.65, 0.72));
    paint_disc(&mut image, 200.0, 200.0, 160.0, hsl_to_rgb(hue, 0.65, 0.72));
    paint_disc(
        &mut image,
        (center_x - eye_gap / 2) as f32 * SCALE,
        eye_y as f32 * SCALE,
        12.0,
        Rgb([0, 0, 0]),
    );
    paint_disc(
        &mut image,
        (center_x + eye_gap / 2) as f32 * SCALE,
        eye_y as f32 * SCALE,
        12.0,
        Rgb([0, 0, 0]),
    );

    let cx = center_x as f32;
    let transformed = |x: f32, y: f32| {
        (
            (cx + (x - cx) * 0.8) * SCALE,
            (65.0 + (y - 65.0) * 0.8) * SCALE,
        )
    };
    let width = 4.0 * SCALE * 0.8;
    match (seed >> 33) % 6 {
        0 => paint_line(
            &mut image,
            transformed(cx - 13.0, 65.0),
            transformed(cx + 13.0, 65.0),
            width,
        ),
        1 => paint_curve(
            &mut image,
            transformed(cx - 14.0, 61.0),
            transformed(cx, 75.0),
            transformed(cx + 14.0, 61.0),
            width,
        ),
        2 => {
            paint_line(
                &mut image,
                transformed(cx - 14.0, 60.0),
                transformed(cx, 72.0),
                width,
            );
            paint_line(
                &mut image,
                transformed(cx, 72.0),
                transformed(cx + 14.0, 60.0),
                width,
            );
        }
        3 => {
            paint_line(
                &mut image,
                transformed(cx + 7.0, 59.0),
                transformed(cx - 7.0, 59.0),
                width,
            );
            paint_curve(
                &mut image,
                transformed(cx - 7.0, 59.0),
                transformed(cx, 87.0),
                transformed(cx + 7.0, 59.0),
                width,
            );
        }
        4 => {
            paint_curve(
                &mut image,
                transformed(cx - 16.0, 61.0),
                transformed(cx - 8.0, 72.0),
                transformed(cx, 61.0),
                width,
            );
            paint_curve(
                &mut image,
                transformed(cx, 61.0),
                transformed(cx + 8.0, 72.0),
                transformed(cx + 16.0, 61.0),
                width,
            );
        }
        _ => {
            paint_line(
                &mut image,
                transformed(cx - 14.0, 59.0),
                transformed(cx - 14.0, 69.0),
                width,
            );
            paint_line(
                &mut image,
                transformed(cx - 14.0, 64.0),
                transformed(cx + 14.0, 64.0),
                width,
            );
            paint_line(
                &mut image,
                transformed(cx + 14.0, 59.0),
                transformed(cx + 14.0, 69.0),
                width,
            );
        }
    }

    let resized = DynamicImage::ImageRgb8(image).resize_exact(
        100,
        100,
        image::imageops::FilterType::Lanczos3,
    );
    let mut bytes = Cursor::new(Vec::new());
    resized
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("PNG encoding into memory cannot fail");
    bytes.into_inner()
}

/// actor ID をシードに、背景・顔色・表情を決定論的に生成する。
pub fn fallback_avatar_svg(actor_id: i64) -> String {
    let seed = seeded(actor_id);
    let hue = seed % 360;
    let background_hue = (hue + 90) % 360;
    let horizontal = ((seed >> 9) % 3) as i32 - 1;
    let eye_y = 43 + (((seed >> 17) % 3) as i32 - 1) * 5;
    let eye_gap = 18 + ((seed >> 25) % 3) as i32 * 5;
    let center_x = 50 + horizontal * 9;
    let left_eye = center_x - eye_gap / 2;
    let right_eye = center_x + eye_gap / 2;
    let mouth = match (seed >> 33) % 6 {
        0 => format!(r#"<path d="M{} 65 H{}"/>"#, center_x - 13, center_x + 13),
        1 => format!(
            r#"<path d="M{} 61 Q{} 75 {} 61"/>"#,
            center_x - 14,
            center_x,
            center_x + 14
        ),
        2 => format!(
            r#"<path d="M{} 60 L{} 72 L{} 60"/>"#,
            center_x - 14,
            center_x,
            center_x + 14
        ),
        3 => format!(
            r#"<path d="M{} 59 H{} Q{} 87 {} 59 Z"/>"#,
            center_x + 7,
            center_x - 7,
            center_x,
            center_x + 7
        ),
        4 => format!(
            r#"<path d="M{} 61 Q{} 72 {} 61 Q{} 72 {} 61"/>"#,
            center_x - 16,
            center_x - 8,
            center_x,
            center_x + 8,
            center_x + 16
        ),
        _ => format!(
            r#"<path d="M{} 59 V69 M{} 64 H{} M{} 59 V69"/>"#,
            center_x - 14,
            center_x - 14,
            center_x + 14,
            center_x + 14
        ),
    };

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" fill="hsl({background_hue} 65% 72%)"/><circle cx="50" cy="50" r="40" fill="hsl({hue} 65% 72%)"/><g fill="#000"><circle cx="{left_eye}" cy="{eye_y}" r="3"/><circle cx="{right_eye}" cy="{eye_y}" r="3"/></g><g fill="none" stroke="#000" stroke-width="4" stroke-linecap="round" stroke-linejoin="round" transform="translate({center_x} 65) scale(.8) translate(-{center_x} -65)">{mouth}</g></svg>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_and_varies_by_actor() {
        assert_eq!(fallback_avatar_svg(42), fallback_avatar_svg(42));
        assert_ne!(fallback_avatar_svg(42), fallback_avatar_svg(43));
    }

    #[test]
    fn url_uses_actor_id() {
        assert_eq!(
            fallback_avatar_url("example.com", 42),
            "https://example.com/api/avatars/42?v=5"
        );
    }

    #[test]
    fn png_is_deterministic_and_valid() {
        let first = fallback_avatar_png(42);
        assert_eq!(first, fallback_avatar_png(42));
        assert_ne!(first, fallback_avatar_png(43));
        assert_eq!(&first[..8], b"\x89PNG\r\n\x1a\n");
    }
}
