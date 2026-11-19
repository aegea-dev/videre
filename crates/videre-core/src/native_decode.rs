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
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A HEIC (`.heic`) or a video's first frame (`.mov`, `.mp4`) as a person
/// sees it, shrunk to fit `max_size`: the contract of
/// `heic::decode_via_quicklook`, which calls this where QuickLook is missing.
pub fn decode(path: &Path, max_size: Option<u32>) -> anyhow::Result<DynamicImage> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let is_heic = match ext.as_str() {
        "heic" => true,
        "mov" | "mp4" => false,
        _ => {
            return Err(
                anyhow::anyhow!("{} is not a HEIC or a video", path.display())
                    .context(ErrorKind::DecodeFailed),
            )
        }
    };
    // The QuickLook limits apply as they would to qlmanage: at most as many
    // heavy decodes at once, and the same ceiling, so a dead drive or a stuck
    // decode is reported as unavailable instead of hanging the caller.
    let _permit = crate::heic::qlmanage_semaphore().acquire();
    let owned = path.to_path_buf();
    let decoded = crate::io_timeout::run_with_timeout(crate::heic::QLMANAGE_TIMEOUT, move || {
        std::panic::catch_unwind(|| -> anyhow::Result<DynamicImage> {
            if is_heic {
                let bytes = std::fs::read(&owned).map_err(crate::error_kind::from_io)?;
                decode_heic(&bytes)
            } else {
                let file = std::fs::File::open(&owned).map_err(crate::error_kind::from_io)?;
                let len = file.metadata().map_err(crate::error_kind::from_io)?.len();
                decode_video(std::io::BufReader::new(file), len)
            }
        })
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!("decoder panicked").context(ErrorKind::DecodeFailed))
        })
    });
    let img = match decoded {
        Ok(result) => result,
        Err(crate::io_timeout::IoRunError::Deadline(limit)) => Err(anyhow::anyhow!(
            "decoding timed out after {}s (file may be unreachable - is its drive connected?)",
            limit.as_secs()
        )
        .context(ErrorKind::SourceUnavailable)),
        Err(other) => Err(crate::error_kind::from_io(other.into_io_error())),
    }
    .with_context(|| format!("decode {}", path.display()))?;
    Ok(fit(img, max_size))
}

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
        heif_oxide::Pixels::Rgba8(v) => {
            v.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
        }
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

/// The first keyframe of an MP4/MOV's video track, turned as the track's
/// display matrix says, as 8-bit RGB.
fn decode_video<R: Read + Seek>(mut reader: R, len: u64) -> anyhow::Result<DynamicImage> {
    // A first read, so an unreadable file reports its I/O failure rather
    // than a missing `moov`.
    let mut head = [0u8; 8];
    reader
        .read_exact(&mut head)
        .and_then(|_| reader.seek(SeekFrom::Start(0)))
        .map_err(crate::error_kind::from_io)?;
    let moov = crate::video_probe::read_moov(&mut reader, len)
        .context(ErrorKind::DecodeFailed)
        .context("no readable moov box")?;
    // `re_mp4` is shown an `ftyp` and this `moov` only: it insists on an
    // `ftyp`, which iPhone Live Photo clips lack, and it loops forever on a
    // classic QuickTime `udta` (see `parser_view`). Sample offsets are
    // absolute in the file, so they still point into `reader`.
    let mut view = FTYP.to_vec();
    view.extend(boxed(b"moov", &parser_view(&moov)));
    let mp4 = re_mp4::Mp4::read_bytes(&view).map_err(mp4_error)?;
    let track = mp4
        .tracks()
        .values()
        .find(|t| t.kind == Some(re_mp4::TrackKind::Video))
        .context(ErrorKind::DecodeFailed)
        .context("no H.264 or HEVC video track")?;
    let sample = track
        .samples
        .iter()
        .find(|s| s.is_sync)
        .context(ErrorKind::DecodeFailed)
        .context("no keyframe")?;
    let mut data = vec![0u8; sample.size as usize];
    reader
        .seek(SeekFrom::Start(sample.offset))
        .and_then(|_| reader.read_exact(&mut data))
        .map_err(crate::error_kind::from_io)?;
    let trak = track.trak(&mp4);
    let frame = match &trak.mdia.minf.stbl.stsd.contents {
        re_mp4::StsdBoxContent::Hvc1(c) | re_mp4::StsdBoxContent::Hev1(c) => {
            let cfg = &c.hvcc.contents;
            let sets = cfg
                .arrays
                .iter()
                .flat_map(|a| a.nalus.iter().map(|n| n.data.as_slice()));
            hevc_frame(sets, cfg.length_size_minus_one + 1, &data)
        }
        re_mp4::StsdBoxContent::Avc1(c) => {
            let cfg = &c.avcc.contents;
            let sets = cfg
                .sequence_parameter_sets
                .iter()
                .chain(&cfg.picture_parameter_sets);
            avc_frame(
                sets.map(|n| n.bytes.as_slice()),
                cfg.length_size_minus_one + 1,
                &data,
            )
        }
        _ => Err(anyhow::anyhow!("unsupported video codec")),
    }
    .map_err(|e| e.context(ErrorKind::DecodeFailed))?;
    let m = &trak.tkhd.matrix;
    Ok(
        match (m.a.signum(), m.b.signum(), m.c.signum(), m.d.signum()) {
            (0, 1, -1, 0) => frame.rotate90(),
            (-1, 0, 0, -1) => frame.rotate180(),
            (0, -1, 1, 0) => frame.rotate270(),
            _ => frame,
        },
    )
}

/// A container error, kept as an I/O failure when it was one: a disconnected
/// drive must not be recorded as an undecodable file.
fn mp4_error(e: re_mp4::Error) -> anyhow::Error {
    match e {
        re_mp4::Error::Io(io) => crate::error_kind::from_io(io),
        other => anyhow::anyhow!("{other}").context(ErrorKind::DecodeFailed),
    }
}

/// The `ftyp` given to `re_mp4`.
const FTYP: [u8; 16] = *b"\0\0\0\x10ftypqt  \0\0\0\0";

/// A box of `typ` around `payload`; a `moov` is far below 4 GB
/// (`video_probe` caps what it reads).
fn boxed(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(typ);
    out.extend_from_slice(payload);
    out
}

/// `moov`'s payload without its `udta` and `meta` boxes, at the top and in
/// each `trak`: user data no frame needs, and where classic QuickTime may end
/// a list with a 32-bit zero that `re_mp4` reads as a box of size 0 and loops
/// on forever (older cameras' Motion JPEG and Dolby Vision clips).
fn parser_view(moov: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(moov.len());
    for (typ, payload) in crate::video_meta::children(moov) {
        match &typ {
            b"udta" | b"meta" => {}
            b"trak" => out.extend(boxed(b"trak", &parser_view(payload))),
            _ => out.extend(boxed(&typ, payload)),
        }
    }
    out
}

/// Length-prefixed NAL units, as MP4 stores them.
fn nal_units(data: &[u8], length_size: u8) -> impl Iterator<Item = &[u8]> {
    let n = length_size as usize;
    let mut at = 0;
    std::iter::from_fn(move || {
        let len = data
            .get(at..at + n)?
            .iter()
            .fold(0usize, |a, &b| a << 8 | b as usize);
        let unit = data.get(at + n..at + n + len)?;
        at += n + len;
        Some(unit)
    })
}

fn hevc_frame<'a>(
    sets: impl Iterator<Item = &'a [u8]>,
    length_size: u8,
    sample: &'a [u8],
) -> anyhow::Result<DynamicImage> {
    let mut dec = rust_h265::Decoder::new().with_display_order();
    let nals = sets.chain(nal_units(sample, length_size));
    for nal in nals.filter_map(rust_h265::parse_nal) {
        dec.decode_nal(&nal).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    let f = dec
        .next_output()
        .or_else(|| dec.flush())
        .context("no frame decoded")?;
    anyhow::ensure!(
        f.chroma_format_idc == 1,
        "chroma format {} is not 4:2:0",
        f.chroma_format_idc
    );
    let to8 = |p: &rust_h265::PixelData| match p {
        rust_h265::PixelData::U8(v) => v.clone(),
        rust_h265::PixelData::U16(v) => v.iter().map(|&s| (s >> (f.bit_depth - 8)) as u8).collect(),
    };
    yuv420_to_rgb(f.width, f.height, &to8(&f.y), &to8(&f.u), &to8(&f.v))
}

fn avc_frame<'a>(
    sets: impl Iterator<Item = &'a [u8]>,
    length_size: u8,
    sample: &'a [u8],
) -> anyhow::Result<DynamicImage> {
    let mut annex_b = Vec::with_capacity(sample.len() + 256);
    for unit in sets.chain(nal_units(sample, length_size)) {
        annex_b.extend_from_slice(&[0, 0, 0, 1]);
        annex_b.extend_from_slice(unit);
    }
    let mut dec = rust_h264::decoder::Decoder::new();
    let mut frame = None;
    for nal in rust_h264::nal::parse_annex_b(&annex_b) {
        if let Some(f) = dec.decode_nal(&nal).map_err(|e| anyhow::anyhow!("{e:?}"))? {
            frame.get_or_insert(f);
        }
    }
    let f = frame.or_else(|| dec.flush()).context("no frame decoded")?;
    anyhow::ensure!(
        f.chroma_format == rust_h264::decoder::ChromaFormat::Yuv420,
        "chroma format is not 4:2:0"
    );
    let to8 = |p: &[u8]| -> Vec<u8> {
        if f.bit_depth > 8 {
            let shift = f.bit_depth - 8;
            p.chunks_exact(2)
                .map(|s| (u16::from_le_bytes([s[0], s[1]]) >> shift) as u8)
                .collect()
        } else {
            p.to_vec()
        }
    };
    yuv420_to_rgb(f.width, f.height, &to8(&f.y), &to8(&f.u), &to8(&f.v))
}

/// Limited-range 4:2:0 to RGB: BT.709 for HD and up, BT.601 below, the usual
/// assumption when a stream's colour signalling is not read.
fn yuv420_to_rgb(w: u32, h: u32, y: &[u8], u: &[u8], v: &[u8]) -> anyhow::Result<DynamicImage> {
    let ch = h.div_ceil(2).max(1);
    let cw = (u.len() as u32 / ch).max(1);
    let planes = yuv::YuvPlanarImage {
        y_plane: y,
        y_stride: w,
        u_plane: u,
        u_stride: cw,
        v_plane: v,
        v_stride: cw,
        width: w,
        height: h,
    };
    let matrix = if h >= 720 {
        yuv::YuvStandardMatrix::Bt709
    } else {
        yuv::YuvStandardMatrix::Bt601
    };
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    yuv::yuv420_to_rgb(&planes, &mut rgb, w * 3, yuv::YuvRange::Limited, matrix)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    image::RgbImage::from_raw(w, h, rgb)
        .map(DynamicImage::ImageRgb8)
        .context("frame pixels do not match its size")
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

    fn video(bytes: Vec<u8>) -> anyhow::Result<DynamicImage> {
        let len = bytes.len() as u64;
        decode_video(std::io::Cursor::new(bytes), len)
    }

    #[test]
    fn an_h264_video_decodes_its_first_frame_in_colour() {
        let img = video(fixture("red_1s.mp4")).unwrap();
        assert_eq!(img.dimensions(), (64, 64));
        let p = img.get_pixel(32, 32).0;
        assert!(p[0] > 200 && p[1] < 60 && p[2] < 60, "not red: {p:?}");
    }

    #[test]
    fn a_video_is_turned_by_its_display_matrix() {
        // Stored 64x32 with a quarter-turn display matrix: shown 32x64.
        let img = video(fixture("testsrc_64x32_rot90.mp4")).unwrap();
        assert_eq!(img.dimensions(), (32, 64));
    }

    #[test]
    fn a_video_without_an_ftyp_box_still_decodes() {
        // As an iPhone Live Photo .mov: no ftyp. Renaming the box to `free`
        // keeps every offset in the file valid.
        let mut bytes = fixture("red_1s.mp4");
        assert_eq!(&bytes[4..8], b"ftyp");
        bytes[4..8].copy_from_slice(b"free");
        assert_eq!(video(bytes).unwrap().dimensions(), (64, 64));
    }

    fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(payload);
        v
    }

    /// Classic QuickTime may end a `udta` list with a 32-bit zero, as older
    /// cameras' Motion JPEG and Dolby Vision clips do. `re_mp4` reads it as a
    /// box of size 0 and loops forever when another box follows, so it is
    /// never shown `udta`.
    #[test]
    fn a_quicktime_udta_terminator_does_not_hang() {
        let mut bytes = fixture("red_1s.mp4");
        // Retype the original moov `free` (every offset stays valid) and
        // append a copy carrying the terminated udta, then one more box.
        let mut at = 0;
        let moov = loop {
            let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            if &bytes[at + 4..at + 8] == b"moov" {
                break at..at + size;
            }
            at += size;
        };
        let mut payload = bytes[moov.start + 8..moov.end].to_vec();
        let mut udta = bx(b"\xa9cmt", b"\0\x0a\0\0Gokce cam");
        udta.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend(bx(b"udta", &udta));
        bytes[moov.start + 4..moov.start + 8].copy_from_slice(b"free");
        bytes.extend(bx(b"moov", &payload));
        bytes.extend(bx(b"free", &[0; 8]));
        let start = std::time::Instant::now();
        assert_eq!(video(bytes).unwrap().dimensions(), (64, 64));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    /// A codec neither decoder reads (Motion JPEG, Dolby Vision `dmb1`) is
    /// refused at once as undecodable.
    #[test]
    fn an_unsupported_codec_is_refused_at_once() {
        let mut bytes = fixture("red_1s.mp4");
        // The sample entry's type, after `stsd` (`ftyp` lists avc1 as a brand).
        let stsd = bytes.windows(4).position(|w| w == b"stsd").unwrap();
        let at = stsd + bytes[stsd..].windows(4).position(|w| w == b"avc1").unwrap();
        bytes[at..at + 4].copy_from_slice(b"jpeg");
        let start = std::time::Instant::now();
        let err = video(bytes).unwrap_err();
        assert_eq!(ErrorKind::in_chain(&err), Some(ErrorKind::DecodeFailed));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn an_audio_only_video_fails_as_a_decode_failure() {
        let err = video(fixture("audio_only.mov")).unwrap_err();
        assert_eq!(ErrorKind::in_chain(&err), Some(ErrorKind::DecodeFailed));
    }

    /// The rotation's direction, which the displayed size cannot show: the
    /// frame matches what QuickLook renders for the same clip.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_turned_video_matches_quicklook() {
        let path = format!(
            "{}/../videre/tests/fixtures/testsrc_64x32_rot90.mp4",
            env!("CARGO_MANIFEST_DIR")
        );
        let ours = video(std::fs::read(&path).unwrap()).unwrap();
        let ql =
            crate::heic::decode_via_quicklook(std::path::Path::new(&path), "native-test", None)
                .unwrap();
        let grey = |i: &DynamicImage| i.resize_exact(16, 32, FilterType::Triangle).to_luma8();
        let mad = |i: &DynamicImage| {
            grey(i)
                .pixels()
                .zip(grey(&ql).pixels())
                .map(|(x, y)| (x.0[0] as f64 - y.0[0] as f64).abs())
                .sum::<f64>()
                / 512.0
        };
        let (right, wrong) = (mad(&ours), mad(&ours.rotate180()));
        assert!(right < 10.0, "differs from QuickLook by {right:.1}");
        assert!(
            wrong > 3.0 * right,
            "a half turn would match as well ({wrong:.1})"
        );
    }

    /// A fixture copied under a Turkish name, as files in a real library are.
    fn copied(name: &str, as_name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(as_name);
        std::fs::write(&path, fixture(name)).unwrap();
        (dir, path)
    }

    #[test]
    fn decode_reads_a_heic_by_path_and_fits_it() {
        let (_dir, path) = copied("heic/grid_rot90.heic", "Gökçe_portre.HEIC");
        assert_eq!(decode(&path, Some(1024)).unwrap().dimensions(), (768, 1024));
    }

    #[test]
    fn decode_reads_a_video_by_path() {
        let (_dir, path) = copied("testsrc_64x32_rot90.mp4", "Işıl_doğum_günü.mov");
        assert_eq!(decode(&path, None).unwrap().dimensions(), (32, 64));
    }

    #[test]
    fn a_missing_file_fails_as_unavailable_not_as_undecodable() {
        let err = decode(Path::new("/nonexistent/Özgür.heic"), None).unwrap_err();
        assert_eq!(
            ErrorKind::in_chain(&err),
            Some(ErrorKind::SourceUnavailable)
        );
    }

    #[test]
    fn an_unknown_extension_is_refused() {
        let (_dir, path) = copied("red_1s.mp4", "Gökçe.avi");
        let err = decode(&path, None).unwrap_err();
        assert_eq!(ErrorKind::in_chain(&err), Some(ErrorKind::DecodeFailed));
    }

    /// Every HEIC, MOV and MP4 under `VIDERE_NATIVE_DECODE_DIR`, decoded, for
    /// checking this backend (or its replacement) against a real folder by
    /// hand, read-only:
    /// `VIDERE_NATIVE_DECODE_DIR=<dir> cargo test -p videre-core native_decode -- --ignored`
    #[test]
    #[ignore]
    fn decodes_every_file_in_a_real_folder() {
        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ["heic", "mov", "mp4"].contains(&ext.to_lowercase().as_str()) {
                        out.push(path);
                    }
                }
            }
        }
        let dir = std::env::var("VIDERE_NATIVE_DECODE_DIR").expect("VIDERE_NATIVE_DECODE_DIR");
        let mut files = Vec::new();
        walk(Path::new(&dir), &mut files);
        let mut failed = Vec::new();
        for path in &files {
            if let Err(e) = decode(path, Some(1024)) {
                failed.push(format!(
                    "{:?} {}: {e:#}",
                    ErrorKind::in_chain(&e),
                    path.display()
                ));
            }
        }
        eprintln!("decoded {} of {}", files.len() - failed.len(), files.len());
        for line in &failed {
            eprintln!("  {line}");
        }
        assert!(
            failed.iter().all(|l| l.starts_with("Some(DecodeFailed)")),
            "a failure other than an undecodable file"
        );
    }

    #[test]
    fn fit_shrinks_to_the_longer_side_and_never_enlarges() {
        let img = DynamicImage::new_rgb8(1536, 2048);
        assert_eq!(fit(img.clone(), Some(512)).dimensions(), (384, 512));
        assert_eq!(fit(img.clone(), Some(4096)).dimensions(), (1536, 2048));
        assert_eq!(fit(img, None).dimensions(), (1536, 2048));
    }
}
