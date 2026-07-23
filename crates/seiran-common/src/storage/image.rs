use std::io::Cursor;

use image::{
    codecs::{
        gif::GifDecoder,
        png::PngDecoder,
        webp::{WebPDecoder, WebPEncoder},
    },
    imageops::FilterType,
    metadata::Orientation,
    AnimationDecoder, DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
};
use sha2::{Digest, Sha256};

use super::exif::strip_exif_except_orientation;

/// リサイズ・WebPエンコード（またはアニメーションのパススルー）を経た画像。
pub struct ProcessedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub size: i64,
    pub sha256: String,
    pub blurhash: String,
    pub mime_type: String,
}

/// Exif情報をOrientationタグのみに絞り込んだ、画素は無劣化のままの画像。
/// width/heightはOrientation適用後の論理（表示）サイズ。
pub struct ExifSanitizedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub size: i64,
    pub sha256: String,
    pub blurhash: String,
    pub mime_type: String,
}

/// `prepare_image` の戻り値。呼び出し元はこれを使って重複排除チェック・保存を行う。
pub enum ImagePipeline {
    /// アニメーション画像（GIF/APNG/WebPアニメ）。元バイト列をそのまま保存する。
    AnimatedPassthrough(ProcessedImage),
    /// 静止画。`original` はExif整理済み無劣化画像（img-parts対応外フォーマットでは
    /// `None`）、`resized` はOrientation適用・リサイズ・WebPエンコード済みの画像。
    Static {
        original: Option<ExifSanitizedImage>,
        resized: ProcessedImage,
    },
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

/// 画像アップロードのパイプライン。
///
/// アニメーション画像は元バイト列をそのまま保存する（従来通り）。
/// 静止画は「Exif整理済み無劣化オリジナル」と「Orientation適用・リサイズ・WebP化」の
/// 2候補を用意し、どちらを採用するかは呼び出し元がDB重複チェック・バイトサイズ比較で決める
/// （ユーザーの画像を不要に劣化させないため、圧縮効果が薄ければ無劣化画像を採用できるようにする）。
pub fn prepare_image(data: &[u8], kind: MediaKind) -> Result<ImagePipeline, ImageProcessingError> {
    let original_format = image::guess_format(data).ok();

    // アニメーション画像（GIF/WebP/APNG）はリサイズ・静止画再エンコードを行わず元のバイト列を
    // そのまま保存する。`image` 0.25 はアニメーションWebP/GIFの「書き出し」に対応していないため、
    // ここで再エンコードすると全フレームが失われ静止画になってしまう（実機で確認された回帰）。
    if let Some(mime) = animated_mime_type(data, original_format) {
        return Ok(ImagePipeline::AnimatedPassthrough(process_animated(data, mime)?));
    }

    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    let format = reader.format();
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);

    let original = format
        .and_then(|format| strip_exif_except_orientation(data, format, orientation))
        .map(|bytes| build_exif_sanitized(bytes, orientation))
        .transpose()?;

    let mut src = DynamicImage::from_decoder(decoder)?;
    src.apply_orientation(orientation);
    let img = resize(&src, kind);
    let resized = encode_webp(&img)?;

    Ok(ImagePipeline::Static { original, resized })
}

/// Exif整理済みの無劣化バイト列から、表示に必要なメタデータ（sha256/blurhash/論理サイズ）を
/// 計算する。`bytes` 自体（保存対象）は再エンコードしない。
fn build_exif_sanitized(
    bytes: Vec<u8>,
    orientation: Orientation,
) -> Result<ExifSanitizedImage, ImageProcessingError> {
    let mut decoded = image::load_from_memory(&bytes)?;
    decoded.apply_orientation(orientation);
    let (width, height) = (decoded.width(), decoded.height());

    let sha256 = hex::encode(Sha256::digest(&bytes));
    let blurhash = compute_blurhash(&decoded, width, height);

    let mime_type = match image::guess_format(&bytes).ok() {
        Some(ImageFormat::Png) => "image/png",
        _ => "image/jpeg",
    }
    .to_owned();

    Ok(ExifSanitizedImage {
        size: bytes.len() as i64,
        data: bytes,
        width,
        height,
        sha256,
        blurhash,
        mime_type,
    })
}

/// Orientation適用済みの画像を、MediaKindごとの最大サイズにリサイズしWebPロスレスエンコードする。
fn encode_webp(img: &DynamicImage) -> Result<ProcessedImage, ImageProcessingError> {
    let (width, height) = (img.width(), img.height());

    let rgba = img.to_rgba8();
    let mut webp_bytes = Vec::new();
    WebPEncoder::new_lossless(&mut webp_bytes).write_image(
        rgba.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;

    let sha256 = hex::encode(Sha256::digest(&webp_bytes));
    let blurhash = compute_blurhash(img, width, height);

    Ok(ProcessedImage {
        size: webp_bytes.len() as i64,
        data: webp_bytes,
        width,
        height,
        sha256,
        blurhash,
        mime_type: "image/webp".to_owned(),
    })
}

/// blurhash 0.2.x にオフバイワンバグがあるため catch_unwind でガードする。
fn compute_blurhash(img: &DynamicImage, width: u32, height: u32) -> String {
    let rgb_raw = img.to_rgb8().as_raw().to_vec();
    std::panic::catch_unwind(move || blurhash::encode(4, 3, width, height, &rgb_raw))
        .unwrap_or_else(|_| Ok(String::new()))
        .unwrap_or_default()
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
/// width/height/blurhashは先頭フレームのみをデコードして算出する（一覧・プレースホルダ表示用）。
fn process_animated(data: &[u8], mime: &'static str) -> Result<ProcessedImage, ImageProcessingError> {
    let sha256 = hex::encode(Sha256::digest(data));

    let first_frame = image::load_from_memory(data)?;
    let (width, height) = (first_frame.width(), first_frame.height());
    let blurhash = compute_blurhash(&first_frame, width, height);

    Ok(ProcessedImage {
        size: data.len() as i64,
        data: data.to_vec(),
        width,
        height,
        sha256,
        blurhash,
        mime_type: mime.to_owned(),
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
    fn prepare_image_preserves_animated_gif_bytes_unchanged() {
        let frames = vec![
            solid_frame(4, 4, [255, 0, 0, 255]),
            solid_frame(4, 4, [0, 255, 0, 255]),
        ];
        let gif_bytes = encode_gif(frames);

        let result = prepare_image(&gif_bytes, MediaKind::Emoji).unwrap();

        match result {
            ImagePipeline::AnimatedPassthrough(p) => {
                assert_eq!(p.mime_type, "image/gif");
                assert_eq!(p.data, gif_bytes, "アニメーションGIFは元のバイト列のまま保存すべき");
            }
            ImagePipeline::Static { .. } => panic!("アニメーションGIFはAnimatedPassthroughになるべき"),
        }
    }

    #[test]
    fn prepare_image_reencodes_single_frame_gif_as_webp() {
        let gif_bytes = encode_gif(vec![solid_frame(4, 4, [255, 0, 0, 255])]);

        let result = prepare_image(&gif_bytes, MediaKind::Emoji).unwrap();

        match result {
            ImagePipeline::Static { original, resized } => {
                assert!(original.is_none(), "GIFはimg-parts非対応なのでoriginalはNone");
                assert_eq!(resized.mime_type, "image/webp", "単一フレームGIFは従来通りWebPへ変換される");
                assert_ne!(resized.data, gif_bytes);
            }
            ImagePipeline::AnimatedPassthrough(_) => panic!("単一フレームGIFはStaticになるべき"),
        }
    }

    fn jpeg_with_orientation(w: u32, h: u32, orientation: Orientation) -> Vec<u8> {
        use img_parts::{jpeg::Jpeg, Bytes, ImageEXIF};

        let img = image::RgbImage::from_fn(w, h, |x, _y| {
            image::Rgb([(x * 10) as u8, 100, 200])
        });
        let mut plain = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut plain), ImageFormat::Jpeg)
            .unwrap();

        let mut jpeg = Jpeg::from_bytes(Bytes::copy_from_slice(&plain)).unwrap();
        jpeg.set_exif(Some(Bytes::from(
            super::super::exif::build_orientation_only_exif(orientation),
        )));
        jpeg.encoder().bytes().to_vec()
    }

    #[test]
    fn prepare_image_applies_orientation_and_strips_other_exif() {
        // 横長(6x4)の画像にOrientation=Rotate90(6)を付与 → 表示上は縦長(4x6)になるべき
        let data = jpeg_with_orientation(6, 4, Orientation::Rotate90);

        let result = prepare_image(&data, MediaKind::Post).unwrap();

        match result {
            ImagePipeline::Static { original, resized } => {
                let original = original.expect("JPEGはimg-parts対応");
                assert_eq!(original.width, 4, "Orientation適用後の論理幅");
                assert_eq!(original.height, 6, "Orientation適用後の論理高さ");
                assert_eq!(resized.width, 4);
                assert_eq!(resized.height, 6);

                // 画素は無劣化（物理ピクセルは元のまま6x4）
                let decoded = image::load_from_memory(&original.data).unwrap();
                assert_eq!((decoded.width(), decoded.height()), (6, 4));
            }
            ImagePipeline::AnimatedPassthrough(_) => panic!("静止画のはず"),
        }
    }
}
