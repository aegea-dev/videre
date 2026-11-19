---
title: Credits
description: The open-source projects videre is built on, and where their licenses are.
---

videre is built on the work of many open-source projects. Thank you to the
people who write and maintain them.

**Every release archive includes `THIRD_PARTY_LICENSES.md`**, with the license
of each project compiled into that build. It is generated for each platform
from the exact dependencies of that release.

## Compiled into videre

- **Over 480 Rust crates**, among them [candle](https://github.com/huggingface/candle)
  (search model inference), [ort](https://ort.pyke.io) (face models),
  [rusqlite](https://github.com/rusqlite/rusqlite), [image](https://github.com/image-rs/image),
  [axum](https://github.com/tokio-rs/axum) and [tokio](https://tokio.rs) (the gallery),
  [clap](https://github.com/clap-rs/clap), [rayon](https://github.com/rayon-rs/rayon)
  and [BLAKE3](https://github.com/BLAKE3-team/BLAKE3). On Linux, HEIC and video are
  decoded with [heif-oxide](https://github.com/dan335/heif-oxide), rust_h265,
  rust_h264 and [re_mp4](https://crates.io/crates/re_mp4).
- [ONNX Runtime](https://onnxruntime.ai) (MIT), which runs face detection and
  recognition.
- [SQLite](https://sqlite.org) (public domain), the library database.
- Place names from [GeoNames](https://www.geonames.org)
  ([CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)).
- In the gallery: [MapLibre GL JS](https://maplibre.org),
  [PMTiles](https://github.com/protomaps/PMTiles) and Flickr's
  [justified-layout](https://github.com/flickr/justified-layout).

## Downloaded on first use

- The search and face models, from the Hugging Face hub, each under its own
  license. See [search models](/reference/models/).
- The Map view's world basemap: [OpenStreetMap](https://www.openstreetmap.org/copyright)
  data, "© OpenStreetMap contributors", built by [Protomaps](https://protomaps.com).
