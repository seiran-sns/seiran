use std::io::Cursor;

use image::{
    codecs::{
        gif::GifDecoder,
        jpeg::JpegEncoder,
        png::{PngDecoder, PngEncoder},
        webp::{WebPDecoder, WebPEncoder},
    },
    imageops::FilterType,
    AnimationDecoder, DynamicImage, ImageEncoder, ImageFormat,
};
use sha2::{Digest, Sha256};

pub struct ProcessedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub size: i64,
    pub sha256: String,
    pub blurhash: String,
    pub mime_type: String,
}

pub enum MediaKind {
    Avatar,
    Banner,
    Emoji,
    Post,
}

#[derive(Debug, thiserror::Error)]
pub enum ImageProcessingError {
    #[error("画像デコード/エンコードエラー: {0}")]
    Image(#[from] image::ImageError),
    #[error("blurhash 計算エラー: {0}")]
    Blurhash(String),
}

pub fn process_image(data: &[u8], kind: MediaKind) -> Result<ProcessedImage, ImageProcessingError> {
    let original_format = image::guess_format(data).ok();

    // アニメーション画像（GIF/WebP/APNG）はリサイズ・静止画再エンコードを行わず元のバイト列を
    // そのまま保存する。`image` 0.25 はアニメーションWebP/GIFの「書き出し」に対応していないため、
    // ここで再エンコードすると全フレームが失われ静止画になってしまう（実機で確認された回帰）。
    if let Some(mime) = animated_mime_type(data, original_format) {
        return process_animated(data, mime);
    }

    let src = image::load_from_memory(data)?;
    let img = resize(&src, kind);
    let (width, height) = (img.width(), img.height());

    // WebP lossless encode
    let rgba = img.to_rgba8();
    let mut webp_bytes = Vec::new();
    WebPEncoder::new_lossless(&mut webp_bytes).write_image(
        rgba.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;

    // 元画像より WebP が大きい場合は元形式（Exif 除去済み）で保存する
    let (final_bytes, final_mime) = if webp_bytes.len() > data.len() {
        match try_encode_original_format(&img, original_format) {
            Some((orig_bytes, mime)) => (orig_bytes, mime),
            None => (webp_bytes, "image/webp".to_owned()),
        }
    } else {
        (webp_bytes, "image/webp".to_owned())
    };

    // SHA-256（実際に保存するバイナリのハッシュ）
    let sha256 = hex::encode(Sha256::digest(&final_bytes));

    // blurhash (RGB) — 0.2.x にオフバイワンバグがあるため catch_unwind でガード
    let rgb = img.to_rgb8();
    let rgb_raw = rgb.as_raw().to_vec();
    let hash = std::panic::catch_unwind(move || {
        blurhash::encode(4, 3, width, height, &rgb_raw)
    })
    .unwrap_or_else(|_| Ok(String::new()))
    .unwrap_or_default();

    Ok(ProcessedImage {
        size: final_bytes.len() as i64,
        data: final_bytes,
        width,
        height,
        sha256,
        blurhash: hash,
        mime_type: final_mime,
    })
}

/// `data` がアニメーション画像（複数フレーム）なら、保存すべき MIME タイプを返す。
/// 静止画（アニメーションGIF/WebP/APNGでも単一フレームのもの）は `None`。
fn animated_mime_type(data: &[u8], format: Option<ImageFormat>) -> Option<&'static str> {
    match format {
        Some(ImageFormat::Gif) => {
            let is_animated = GifDecoder::new(Cursor::new(data))
                .ok()
                .map(|d| d.into_frames().take(2).count() >= 2)
                .unwrap_or(false);
            is_animated.then_some("image/gif")
        }
        Some(ImageFormat::WebP) => {
            let is_animated = WebPDecoder::new(Cursor::new(data))
                .ok()
                .map(|d| d.has_animation())
                .unwrap_or(false);
            is_animated.then_some("image/webp")
        }
        Some(ImageFormat::Png) => {
            let is_animated = PngDecoder::new(Cursor::new(data))
                .ok()
                .map(|d| d.is_apng().unwrap_or(false))
                .unwrap_or(false);
            is_animated.then_some("image/png")
        }
        _ => None,
    }
}

/// アニメーション画像を元のバイト列のまま `ProcessedImage` にする。
/// width/height/blurhash は先頭フレームのみをデコードして算出する（一覧・プレースホルダ表示用）。
fn process_animated(data: &[u8], mime: &'static str) -> Result<ProcessedImage, ImageProcessingError> {
    let sha256 = hex::encode(Sha256::digest(data));

    let first_frame = image::load_from_memory(data)?;
    let (width, height) = (first_frame.width(), first_frame.height());
    let rgb_raw = first_frame.to_rgb8().as_raw().to_vec();
    let hash = std::panic::catch_unwind(move || blurhash::encode(4, 3, width, height, &rgb_raw))
        .unwrap_or_else(|_| Ok(String::new()))
        .unwrap_or_default();

    Ok(ProcessedImage {
        size: data.len() as i64,
        data: data.to_vec(),
        width,
        height,
        sha256,
        blurhash: hash,
        mime_type: mime.to_owned(),
    })
}

/// `image` クレートで再エンコードして Exif を除去する。
/// JPEG は quality=85 で再エンコード、PNG は lossless。
/// 対応外の形式（GIF, AVIF など）は None を返す（呼び出し元が WebP にフォールバック）。
fn try_encode_original_format(
    img: &DynamicImage,
    format: Option<ImageFormat>,
) -> Option<(Vec<u8>, String)> {
    match format {
        Some(ImageFormat::Jpeg) => {
            let mut buf = Vec::new();
            let rgb = img.to_rgb8();
            JpegEncoder::new_with_quality(&mut buf, 85)
                .write_image(
                    rgb.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .ok()?;
            Some((buf, "image/jpeg".to_owned()))
        }
        Some(ImageFormat::Png) => {
            let mut buf = Vec::new();
            let rgba = img.to_rgba8();
            PngEncoder::new(&mut buf)
                .write_image(
                    rgba.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .ok()?;
            Some((buf, "image/png".to_owned()))
        }
        _ => None,
    }
}

fn resize(img: &DynamicImage, kind: MediaKind) -> DynamicImage {
    match kind {
        // センタークロップで正方形
        MediaKind::Avatar => img.resize_to_fill(600, 600, FilterType::Lanczos3),
        // 収まる最大サイズに縮小
        MediaKind::Banner => fit_inside(img, 2048, 768),
        // 絵文字用の小サイズ
        MediaKind::Emoji => fit_inside(img, 384, 64),
        // 長辺 2048 以内に収める
        MediaKind::Post => {
            if img.width().max(img.height()) <= 2048 {
                img.clone()
            } else {
                fit_inside(img, 2048, 2048)
            }
        }
    }
}

fn fit_inside(img: &DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    if img.width() <= max_w && img.height() <= max_h {
        return img.clone();
    }
    img.resize(max_w, max_h, FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{codecs::gif::GifEncoder, Frame, ImageBuffer, Rgba};

    fn solid_frame(w: u32, h: u32, color: [u8; 4]) -> Frame {
        let buf = ImageBuffer::from_pixel(w, h, Rgba(color));
        Frame::new(buf)
    }

    fn encode_gif(frames: Vec<Frame>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.encode_frames(frames).unwrap();
        drop(encoder);
        bytes
    }

    #[test]
    fn process_image_preserves_animated_gif_bytes_unchanged() {
        let frames = vec![
            solid_frame(4, 4, [255, 0, 0, 255]),
            solid_frame(4, 4, [0, 255, 0, 255]),
        ];
        let gif_bytes = encode_gif(frames);

        let result = process_image(&gif_bytes, MediaKind::Emoji).unwrap();

        assert_eq!(result.mime_type, "image/gif");
        assert_eq!(result.data, gif_bytes, "アニメーションGIFは元のバイト列のまま保存すべき");
    }

    #[test]
    fn process_image_reencodes_single_frame_gif_as_webp() {
        let gif_bytes = encode_gif(vec![solid_frame(4, 4, [255, 0, 0, 255])]);

        let result = process_image(&gif_bytes, MediaKind::Emoji).unwrap();

        assert_eq!(result.mime_type, "image/webp", "単一フレームGIFは従来通りWebPへ変換される");
        assert_ne!(result.data, gif_bytes);
    }
}
