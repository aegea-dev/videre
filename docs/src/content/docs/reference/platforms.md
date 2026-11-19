---
title: Platform support
description: What works on macOS and Linux, and what does not work at all.
---

macOS is the primary platform, and everything works there. videre also builds
and runs on Linux, where HEIC and video are decoded differently.

## Per-command support

| | macOS | Linux |
|-|-------|-------|
| `scan`, `dedupe`, `gallery`, `fix-dates`, `prune`, `locations`, `stats`, `mcp`, `config` | yes | yes |
| `embed`, `search` | yes (GPU) | yes (CPU only) |
| `faces` | yes | yes |
| `watch` | yes | yes |
| HEIC decoding | yes (QuickLook) | yes (built in) |
| Video frame extraction | yes (QuickLook) | yes (built in, first keyframe) |
| HEIC and video scanning, hashing, EXIF | yes | yes |
| File creation time | yes | always empty |

## Intel Macs are not supported

The ONNX Runtime dependency ships no prebuilt binaries for
`x86_64-apple-darwin`, so any build fails with:

```
no prebuilt binaries available for target x86_64-apple-darwin
```

This is not a packaging problem and cannot be worked around by building it
yourself: `cargo install videre` fails identically on an Intel Mac.

## HEIC and video on Linux

macOS decodes HEIC images and video frames through QuickLook. Linux has no
QuickLook, so videre decodes them itself, with nothing to install:

- HEIC: the primary image, turned as the file says, as on macOS.
- Video (`.mov`, `.mp4`): the first keyframe of an H.264 or HEVC clip, turned
  by the clip's rotation. Other codecs, such as Dolby Vision or Motion JPEG,
  are skipped after two attempts, as any undecodable file is.
- HDR clips and photos are shown without tone mapping, so they can look flatter
  than on macOS.

Regular JPEG, PNG, GIF, WebP, BMP and TIFF work identically on both platforms.

## Building on ARM64 Linux

Building from source needs one extra flag. The released binary does not, since
it is already built with it:

```bash
RUSTFLAGS="-C target-feature=+fp16" cargo install videre --locked
```

Without it the build fails with `instruction requires: fullfp16`. Building from
inside a clone of the repository handles this automatically.

x86_64 Linux is unaffected.

:::caution
`cargo check` passes on ARM64 Linux even when `cargo build` fails, because
`check` never runs code generation. A green `check` is not evidence that
`cargo install` works.
:::
