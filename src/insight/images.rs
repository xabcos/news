//! Temporary vision inputs. Published photographs retain their original format and URL.

use crate::{
    fetch::NewsImage,
    media::{self, MIN_IMAGE_BYTES, MIN_IMAGE_DIMENSION},
    state::AppState,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, codecs::jpeg::JpegEncoder};
use std::{collections::HashMap, io::Cursor};

const ACCEPT: &str = "image/jpeg,image/png,image/webp,image/gif";
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
/// The request asks for `detail: low`; larger inputs only add upload bytes.
const MAX_SIDE: u32 = 1024;

pub(super) async fn prepare(
    state: &AppState,
    images: Vec<(usize, NewsImage)>,
) -> HashMap<usize, String> {
    let total = images.len();
    let jobs = images.into_iter().map(|(index, image)| {
        let state = state.clone();
        move |slot| async move {
            tokio::time::timeout(state.config.image_timeout, async {
                // Negotiate at the origin: the mirrored AVIF is for browser display.
                let origin = image.origin_url.as_deref().unwrap_or(&image.url);
                let (bytes, _) = media::download_image(&state, origin, ACCEPT).await?;
                let max_bytes = state.config.image_max_bytes;
                let data = tokio::task::spawn_blocking(move || {
                    let _slot = slot;
                    let jpeg = normalize_image(&bytes)?;
                    (jpeg.len() as u64 <= max_bytes)
                        .then(|| format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)))
                })
                .await
                .ok()
                .flatten()?;
                Some((index, data))
            })
            .await
            .ok()
            .flatten()
        }
    });
    let prepared: HashMap<_, _> = media::batch(state, jobs)
        .await
        .into_iter()
        .flatten()
        .collect();
    tracing::info!(
        available = prepared.len(),
        total,
        "prepared AI insight images"
    );
    prepared
}

/// Validate the complete file and send a static JPEG directly, without a second upload.
fn normalize_image(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < MIN_IMAGE_BYTES {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if width < MIN_IMAGE_DIMENSION
        || height < MIN_IMAGE_DIMENSION
        || decoder.total_bytes() > MAX_DECODE_BYTES
    {
        return None;
    }
    let orientation = decoder.orientation().ok()?;
    let mut decoded = DynamicImage::from_decoder(decoder).ok()?;
    decoded.apply_orientation(orientation);
    if decoded.width() > MAX_SIDE || decoded.height() > MAX_SIDE {
        decoded = decoded.thumbnail(MAX_SIDE, MAX_SIDE);
    }
    let decoded = if decoded.color().has_alpha() {
        let rgba = decoded.into_rgba8();
        image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let pixel = rgba.get_pixel(x, y);
            let alpha = u16::from(pixel[3]);
            image::Rgb(std::array::from_fn(|channel| {
                ((u16::from(pixel[channel]) * alpha + 255 * (255 - alpha)) / 255) as u8
            }))
        })
    } else {
        decoded.into_rgb8()
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&decoded)
        .ok()?;
    Some(jpeg)
}

#[cfg(test)]
pub(super) fn fixture_image(format: image::ImageFormat, size: u32) -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(size, size, |x, y| {
        let n = x
            .wrapping_mul(1_664_525)
            .wrapping_add(y.wrapping_mul(1_013_904_223));
        image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(pixels)
        .write_to(&mut bytes, format)
        .unwrap();
    bytes.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_supported_images_into_valid_static_jpeg() {
        for format in [
            image::ImageFormat::Jpeg,
            image::ImageFormat::Png,
            image::ImageFormat::WebP,
            image::ImageFormat::Gif,
        ] {
            let bytes = fixture_image(format, 640);
            let normalized = normalize_image(&bytes).expect("valid supported image");
            assert_eq!(
                image::guess_format(&normalized).unwrap(),
                image::ImageFormat::Jpeg
            );
            let decoded = image::load_from_memory(&normalized).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (640, 640));
        }
    }

    #[test]
    fn rejects_placeholders_corrupt_files_and_unsupported_formats() {
        assert!(normalize_image(&fixture_image(image::ImageFormat::Png, 220)).is_none());
        let mut fake = vec![0; 8192];
        fake[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        fake[16..20].copy_from_slice(&640_u32.to_be_bytes());
        fake[20..24].copy_from_slice(&640_u32.to_be_bytes());
        assert!(normalize_image(&fake).is_none());
        let png = fixture_image(image::ImageFormat::Png, 640);
        assert!(normalize_image(&png[..4096]).is_none());
        assert!(normalize_image(&b"<html>Access denied</html>".repeat(200)).is_none());
        fake[..16].copy_from_slice(b"\0\0\0\x1cftypavif\0\0\0\0");
        assert!(normalize_image(&fake).is_none());
    }
}
