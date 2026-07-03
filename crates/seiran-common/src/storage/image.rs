use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::PngEncoder,
        webp::WebPEncoder,
    },
    imageops::FilterType,
    DynamicImage, ImageEncoder, ImageFormat,
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
