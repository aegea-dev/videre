---
title: Supported files
description: Which file types videre reads, and what it can do with each.
---

`.jpg` `.jpeg` `.png` `.gif` `.webp` `.bmp` `.tiff` `.heic` `.mov` `.mp4` `.dng`

## What works with what

| Type | Scan & dedupe | EXIF | Search | Faces | Near-duplicate |
|---|---|---|---|---|---|
| jpg, jpeg, tiff | yes | yes | yes | yes | yes |
| png, gif, webp, bmp | yes | no | yes | yes | yes |
| heic | yes | yes | yes | yes | yes |
| mov, mp4 | yes | no | yes, from one frame | no | yes, from one frame |
| dng | yes | yes | no | no | no |

These formats are scanned, hashed and exactly de-duplicated regardless. The gaps
above are about what can be *decoded*, not what is recorded. The near-duplicate
fingerprint is made by [`embed`](/commands/embed/), from the image it decodes
anyway, so a file has one once it has been embedded.

`.dng` is skipped for search because no DNG decoder is available. Its EXIF
metadata is still read.

See [platform support](/reference/platforms/) for why HEIC and video need macOS.

## File type detection

Types are identified by the file's actual leading bytes, not its name, so a
mislabeled file is still handled correctly. This costs nothing extra, since the
bytes are already being read to hash the file.

A `.png` that is really a JPEG is treated as a JPEG. A `.dng` reports as TIFF,
which it genuinely is, so DNG is excluded from search by extension rather than
by type.

macOS writes AppleDouble metadata files (`._photo.jpg`) beside the originals on
volumes that cannot store extended attributes, such as exFAT. videre recognises
them by their contents, not their name, and leaves them out of the library, so
a real photo that happens to be named `._something.jpg` is still scanned.
The first scan reads only their 26-byte header. The result is remembered
separately from the media index: subsequent scan and watch passes check only
the file's size and modification time. A change to either makes videre check
the contents again; `scan --force` rechecks them even if both are unchanged.
Both newly recognised and unchanged AppleDouble files count as skipped in the
scan summary.

Rows recorded by an earlier version stay in the database;
[`prune`](/commands/prune/) does not remove them, because it only drops a row
when its file is gone.

When the bytes match nothing recognised, the file is recorded as
`application/octet-stream` rather than left empty. That is a real answer, not a
failure: it records that the question was asked and settled, which is what stops
an incremental [`scan`](/commands/scan/) reopening the same file on every run. Such a file is still processed, falling back to its extension.

Files scanned before type detection existed have no recorded type until you
re-scan; those also fall back to the extension.

## EXIF fields

EXIF is read from `jpg`, `jpeg`, `tiff`, `heic` and `dng`. Fields are empty when
the file carries no EXIF data.

| Field | Notes |
|-------|-------|
| Date taken | `DateTimeOriginal`, camera-local with no timezone |
| GPS latitude | Decimal degrees, negative is South |
| GPS longitude | Decimal degrees, negative is West |
| Width, height | Pixel dimensions |

Dates of `0000-00-00`, which cameras with an unset clock produce, are discarded
rather than stored.

## Near-duplicate detection for video

For `.mov` and `.mp4`, the fingerprint comes from a single poster frame, not the
video content. So it catches re-encodes and trims that keep the opening frame,
but not a trim that cuts it.

That output is review-only and never reaches
[`videre dedupe`](/commands/dedupe/)'s pipeable output, so a false match costs
you a noisy group in the report, never a wrong deletion.
