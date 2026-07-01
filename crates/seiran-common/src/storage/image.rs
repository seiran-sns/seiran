use image::{
    codecs::webp::WebPEncoder,
    imageops::FilterType,
    DynamicImage, ImageEncoder,
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

    // SHA-256
    let sha256 = hex::encode(Sha256::digest(&webp_bytes));

    // blurhash (RGB)
    let rgb = img.to_rgb8();
    let hash = blurhash::encode(4, 3, width, height, rgb.as_raw())
        .map_err(|e| ImageProcessingError::Blurhash(e.to_string()))?;

    Ok(ProcessedImage {
        size: webp_bytes.len() as i64,
        data: webp_bytes,
        width,
        height,
        sha256,
        blurhash: hash,
        mime_type: "image/webp".to_owned(),
    })
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
