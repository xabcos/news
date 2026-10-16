//! Temporary vision inputs. Published photographs retain their original format and URL.

use crate::{fetch::NewsImage, media, state::AppState};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::HashMap;

const ACCEPT: &str = "image/jpeg,image/png,image/webp,image/gif";
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
                    let jpeg = media::static_jpeg(&bytes, MAX_SIDE)?;
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

#[cfg(test)]
pub(super) fn fixture_image(format: image::ImageFormat, size: u32) -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(size, size, |x, y| {
        let n = x
            .wrapping_mul(1_664_525)
            .wrapping_add(y.wrapping_mul(1_013_904_223));
        image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels)
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
            let normalized = media::static_jpeg(&bytes, MAX_SIDE).expect("valid supported image");
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
        assert!(
            media::static_jpeg(&fixture_image(image::ImageFormat::Png, 220), MAX_SIDE).is_none()
        );
        let mut fake = vec![0; 8192];
        fake[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        fake[16..20].copy_from_slice(&640_u32.to_be_bytes());
        fake[20..24].copy_from_slice(&640_u32.to_be_bytes());
        assert!(media::static_jpeg(&fake, MAX_SIDE).is_none());
        let png = fixture_image(image::ImageFormat::Png, 640);
        assert!(media::static_jpeg(&png[..4096], MAX_SIDE).is_none());
        assert!(media::static_jpeg(&b"<html>Access denied</html>".repeat(200), MAX_SIDE).is_none());
        fake[..16].copy_from_slice(b"\0\0\0\x1cftypavif\0\0\0\0");
        assert!(media::static_jpeg(&fake, MAX_SIDE).is_none());
    }
}
