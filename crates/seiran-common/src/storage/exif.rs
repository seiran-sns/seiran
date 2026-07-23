use image::metadata::Orientation;
use img_parts::{jpeg::Jpeg, png::Png, Bytes, ImageEXIF};

/// Orientationタグのみを含む最小のExif(TIFF)バイト列を生成する（26バイト固定長）。
/// "Exif\0\0" プレフィックスは含まない（JPEGのAPP1セグメントはimg-partsが、PNGの
/// eXIfチャンクは仕様自体がプレフィックス無しの生TIFFを期待するため）。
pub fn build_orientation_only_exif(orientation: Orientation) -> Vec<u8> {
    let mut buf = Vec::with_capacity(26);
    buf.extend_from_slice(b"II"); // byte order: little endian
    buf.extend_from_slice(&42u16.to_le_bytes()); // TIFF magic number
    buf.extend_from_slice(&8u32.to_le_bytes()); // offset to first IFD
    buf.extend_from_slice(&1u16.to_le_bytes()); // number of IFD entries
    buf.extend_from_slice(&0x0112u16.to_le_bytes()); // tag: Orientation
    buf.extend_from_slice(&3u16.to_le_bytes()); // type: SHORT
    buf.extend_from_slice(&1u32.to_le_bytes()); // count
    buf.extend_from_slice(&u16::from(orientation.to_exif()).to_le_bytes()); // value
    buf.extend_from_slice(&0u16.to_le_bytes()); // padding to fill the 4-byte value slot
    buf.extend_from_slice(&0u32.to_le_bytes()); // next IFD offset (none)
    buf
}

/// JPEG/PNGについて、画素を一切再エンコードせず、Exif情報をOrientationタグのみに
/// 絞り込んだバイト列を返す（GPS等の他のExifタグは削除する）。
/// Orientationが `NoTransforms` の場合はExifセグメント/チャンク自体を削除する。
/// 対応外フォーマット（JPEG/PNG以外）やコンテナ解析失敗時は `None` を返す。
pub fn strip_exif_except_orientation(
    data: &[u8],
    format: image::ImageFormat,
    orientation: Orientation,
) -> Option<Vec<u8>> {
    let new_exif = (orientation != Orientation::NoTransforms)
        .then(|| Bytes::from(build_orientation_only_exif(orientation)));

    match format {
        image::ImageFormat::Jpeg => {
            let mut jpeg = Jpeg::from_bytes(Bytes::copy_from_slice(data)).ok()?;
            jpeg.set_exif(new_exif);
            Some(jpeg.encoder().bytes().to_vec())
        }
        image::ImageFormat::Png => {
            let mut png = Png::from_bytes(Bytes::copy_from_slice(data)).ok()?;
            png.set_exif(new_exif);
            Some(png.encoder().bytes().to_vec())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ORIENTATIONS: [Orientation; 8] = [
        Orientation::NoTransforms,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::FlipHorizontal,
        Orientation::FlipVertical,
        Orientation::Rotate90FlipH,
        Orientation::Rotate270FlipH,
    ];

    #[test]
    fn build_orientation_only_exif_roundtrips_through_image_crate() {
        for orientation in ALL_ORIENTATIONS {
            let bytes = build_orientation_only_exif(orientation);
            let restored = Orientation::from_exif_chunk(&bytes)
                .unwrap_or_else(|| panic!("failed to parse orientation {orientation:?}"));
            assert_eq!(restored, orientation);
        }
    }

    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 100, 50]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .unwrap();
        buf
    }

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn strip_exif_except_orientation_jpeg_keeps_pixels_and_orientation_only() {
        let original = tiny_jpeg();

        // まずOrientationタグを含むExifを埋め込んだJPEGを作る
        let mut jpeg = Jpeg::from_bytes(Bytes::copy_from_slice(&original)).unwrap();
        jpeg.set_exif(Some(Bytes::from(build_orientation_only_exif(
            Orientation::Rotate90,
        ))));
        let with_exif = jpeg.encoder().bytes().to_vec();

        let stripped =
            strip_exif_except_orientation(&with_exif, image::ImageFormat::Jpeg, Orientation::Rotate90)
                .expect("jpeg should be supported");

        // Orientationタグは残っている
        let parsed = Jpeg::from_bytes(Bytes::copy_from_slice(&stripped)).unwrap();
        let exif = parsed.exif().expect("orientation exif should remain");
        assert_eq!(
            Orientation::from_exif_chunk(&exif),
            Some(Orientation::Rotate90)
        );

        // 画素データは無劣化（デコード結果が一致）
        let original_pixels = image::load_from_memory(&original).unwrap().to_rgb8();
        let stripped_pixels = image::load_from_memory(&stripped).unwrap().to_rgb8();
        assert_eq!(original_pixels, stripped_pixels);
    }

    #[test]
    fn strip_exif_except_orientation_no_transforms_removes_exif_segment() {
        let original = tiny_jpeg();
        let mut jpeg = Jpeg::from_bytes(Bytes::copy_from_slice(&original)).unwrap();
        jpeg.set_exif(Some(Bytes::from(b"dummy-exif-with-gps-etc".to_vec())));
        let with_exif = jpeg.encoder().bytes().to_vec();

        let stripped = strip_exif_except_orientation(
            &with_exif,
            image::ImageFormat::Jpeg,
            Orientation::NoTransforms,
        )
        .unwrap();

        let parsed = Jpeg::from_bytes(Bytes::copy_from_slice(&stripped)).unwrap();
        assert!(parsed.exif().is_none());
    }

    #[test]
    fn strip_exif_except_orientation_png_keeps_pixels_and_orientation_only() {
        let original = tiny_png();
        let mut png = Png::from_bytes(Bytes::copy_from_slice(&original)).unwrap();
        png.set_exif(Some(Bytes::from(build_orientation_only_exif(
            Orientation::Rotate180,
        ))));
        let with_exif = png.encoder().bytes().to_vec();

        let stripped =
            strip_exif_except_orientation(&with_exif, image::ImageFormat::Png, Orientation::Rotate180)
                .expect("png should be supported");

        let parsed = Png::from_bytes(Bytes::copy_from_slice(&stripped)).unwrap();
        let exif = parsed.exif().expect("orientation exif should remain");
        assert_eq!(
            Orientation::from_exif_chunk(&exif),
            Some(Orientation::Rotate180)
        );

        let original_pixels = image::load_from_memory(&original).unwrap().to_rgb8();
        let stripped_pixels = image::load_from_memory(&stripped).unwrap().to_rgb8();
        assert_eq!(original_pixels, stripped_pixels);
    }

    #[test]
    fn strip_exif_except_orientation_unsupported_format_returns_none() {
        // GIFはimg-parts非対応
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Gif)
            .unwrap();

        assert!(strip_exif_except_orientation(&buf, image::ImageFormat::Gif, Orientation::NoTransforms).is_none());
    }
}
