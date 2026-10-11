//! The size a file is shown at, which is what `file_hashes.width` and
//! `height` hold: the gallery lays tiles out by it, and XMP face regions are
//! measured against it, as face boxes are on the displayed canvas.
//!
//! A camera stores a portrait shot landscape and marks it to be shown turned:
//! a HEIC by its `irot`, a JPEG (and PNG, WebP, TIFF) by its EXIF Orientation,
//! a video by its track's display matrix. The stored size turned by that mark
//! is the displayed size. Every reader here reads headers only.

use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// The displayed size of the open file, or `None` when its header cannot be
/// read. `mime` is the scanned type; HEIC is told by it, not the extension.
pub fn of_reader<R: Read + Seek>(
    reader: &mut R,
    len: u64,
    mime: Option<&str>,
    ext: &str,
) -> Option<(u32, u32)> {
    let mime = videre_core::mime_probe::effective_mime(mime, ext)?;
    reader.seek(SeekFrom::Start(0)).ok()?;
    if mime == "image/heic" {
        return crate::heif_rotate::displayed_size(reader, len);
    }
    if videre_core::mime_probe::is_video_mime(mime) {
        let v = videre_core::video_meta::read_file_in_progress(reader);
        return v.width.zip(v.height);
    }
    videre_core::image_decode::displayed_size_reader(BufReader::new(reader))
}

/// [`of_reader`] for a path, opening it read-only.
pub fn of_path(path: &Path, mime: Option<&str>, ext: &str) -> Option<(u32, u32)> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    of_reader(&mut file, len, mime, ext)
}
