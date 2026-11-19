# Third-party licenses

videre is free software under the Apache License 2.0 (`LICENSE`). It is built
on the work of many other open-source projects, listed here with their
licenses. Thank you to everyone who wrote and maintains them.

This file is generated for each release (`make licenses`) and shipped in every
release archive.

## Compiled into the binary

### ONNX Runtime

Face detection and recognition run on ONNX Runtime, linked statically from the
prebuilt build the `ort` crate supplies (ONNX Runtime 1.28.0).

- Source: https://onnxruntime.ai
- License: MIT. ONNX Runtime's own third-party notices, for the libraries it
  includes, are published with its source.

```
MIT License

Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### SQLite

The library database is SQLite, compiled in through `libsqlite3-sys`.

- Source: https://sqlite.org
- License: public domain (https://sqlite.org/copyright.html)

### GeoNames

Offline place names come from a GeoNames extract bundled by the
`reverse_geocoder` crate.

- Source: GeoNames, https://www.geonames.org
- License: Creative Commons Attribution 4.0
  (https://creativecommons.org/licenses/by/4.0/)

## Downloaded on first use

These are not part of the binary; videre fetches them when a feature first
needs them, and their licenses apply to that copy.

- The image search model (SigLIP 2, Google) and the face models (InsightFace
  `buffalo_l`), from the Hugging Face hub, each under its own license.
- The Map view's world basemap: OpenStreetMap data (Open Database License,
  "© OpenStreetMap contributors", shown on the map) built by Protomaps.

