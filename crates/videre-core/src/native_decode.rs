//! In-process HEIC and video-frame decoding, used where macOS QuickLook is not
//! available (Linux). `heic::decode_via_quicklook` calls [`decode`] on Linux;
//! nothing else does.
//!
//! :warning: **This module is the only code that names its crates**
//! (`heif-oxide`, `rust_h265`, `rust_h264`, `re_mp4`, `yuv`). They are young,
//! so the backend is kept replaceable in one step. To remove it: delete this
//! file and its `pub mod`, the five dependency lines in `Cargo.toml`, and the
//! `native_decode::decode` call in `heic::decode_via_quicklook`; set
//! `heic::FRAMES_AVAILABLE` back to macOS only; and gate these tests back to
//! `target_os = "macos"`: `tests/heic_rotate.rs`, the poster, HEIC-cache and
//! rotated-HEIC routes in `tests/gallery_routes.rs`, and the unit tests in
//! `heic.rs`, `videre-ml`'s `preprocess.rs` and `videre-api`'s `images.rs`.
//!
//! It is compiled everywhere so its tests run on a Mac too, where they can be
//! checked against QuickLook.

use crate::error_kind::ErrorKind;
use anyhow::Context as _;
use image::{imageops::FilterType, DynamicImage};

/// A HEIC's primary image as a person sees it: `heif-oxide` applies the
/// container's `clap`, `irot` and `imir` and ignores EXIF Orientation, as
/// QuickLook does (see `tests/fixtures/heic/README.md`).
fn decode_heic(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    let decoded = heif_oxide::decode_bytes(bytes)
        .map_err(|e| anyhow::anyhow!("{e}").context(ErrorKind::DecodeFailed))
        .context("decode HEIC")?;
    // 8-bit RGB whatever the source depth, without alpha: what callers encode
    // as JPEG, and what QuickLook's PNG gave them after its alpha was unused.
    let (w, h) = (decoded.width, decoded.height);
    let rgb: Vec<u8> = match decoded.pixels {
        heif_oxide::Pixels::Rgb8(v) => v,
        heif_oxide::Pixels::Rgba8(v) => v
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect(),
        heif_oxide::Pixels::Rgb16(v) => v.iter().map(|&s| (s >> 8) as u8).collect(),
        heif_oxide::Pixels::Rgba16(v) => v
            .chunks_exact(4)
            .flat_map(|p| [(p[0] >> 8) as u8, (p[1] >> 8) as u8, (p[2] >> 8) as u8])
            .collect(),
    };
    image::RgbImage::from_raw(w, h, rgb)
        .map(DynamicImage::ImageRgb8)
        .context(ErrorKind::DecodeFailed)
        .context("HEIC pixels do not match its size")
}

/// Shrunk to fit `max_size` on its longer side, as QuickLook's `-s` does;
/// never enlarged.
fn fit(img: DynamicImage, max_size: Option<u32>) -> DynamicImage {
    match max_size {
        Some(n) if img.width().max(img.height()) > n => img.resize(n, n, FilterType::Triangle),
        _ => img,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../videre/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// Which corner the red marker is in, as (right, bottom).
    fn marker_corner(img: &DynamicImage) -> (bool, bool) {
        let (w, h) = img.dimensions();
        let red = |x: u32, y: u32| {
            let p = img.get_pixel(x, y).0;
            p[0] > 200 && p[1] < 80 && p[2] < 80
        };
        let (x_in, y_in) = (w / 40, h / 40);
        let corners = [
            (false, false, x_in, y_in),
            (true, false, w - 1 - x_in, y_in),
            (false, true, x_in, h - 1 - y_in),
            (true, true, w - 1 - x_in, h - 1 - y_in),
        ];
        let hits: Vec<_> = corners
            .iter()
            .filter(|c| red(c.2, c.3))
            .map(|c| (c.0, c.1))
            .collect();
        assert_eq!(hits.len(), 1, "exactly one red corner, got {hits:?}");
        hits[0]
    }

    #[test]
    fn a_heic_with_irot_decodes_turned_as_quicklook_shows_it() {
        let img = decode_heic(&fixture("heic/grid_rot90.heic")).unwrap();
        assert_eq!(img.dimensions(), (1536, 2048));
        assert_eq!(marker_corner(&img), (true, false));
    }

    #[test]
    fn a_heic_without_irot_ignores_its_exif_orientation() {
        let img = decode_heic(&fixture("heic/grid_no_irot.heic")).unwrap();
        assert_eq!(img.dimensions(), (2048, 1536));
        assert_eq!(marker_corner(&img), (false, false));
    }

    #[test]
    fn a_decoded_heic_is_plain_rgb8() {
        let img = decode_heic(&fixture("heic/grid_rot90.heic")).unwrap();
        assert!(matches!(img, DynamicImage::ImageRgb8(_)));
    }

    #[test]
    fn a_heic_that_is_not_one_fails_as_a_decode_failure() {
        let err = decode_heic(b"not a heic file").unwrap_err();
        assert_eq!(ErrorKind::in_chain(&err), Some(ErrorKind::DecodeFailed));
    }

    #[test]
    fn fit_shrinks_to_the_longer_side_and_never_enlarges() {
        let img = DynamicImage::new_rgb8(1536, 2048);
        assert_eq!(fit(img.clone(), Some(512)).dimensions(), (384, 512));
        assert_eq!(fit(img.clone(), Some(4096)).dimensions(), (1536, 2048));
        assert_eq!(fit(img, None).dimensions(), (1536, 2048));
    }
}
