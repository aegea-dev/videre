# HEIC rotation fixtures

`grid_rot90.heic` is a synthetic test pattern, never a photograph, laid out
the way an iPhone saves a portrait shot:

- the primary item is a 2048x1536 `grid` of twelve 512x512 `hvc1` tiles;
- a 320x240 `hvc1` thumbnail;
- one `irot` property of 270 degrees (counter-clockwise), shared through
  `ipma` by the grid and the thumbnail, so it displays turned 90 degrees
  clockwise;
- EXIF Orientation 6, matching the `irot`.

The stored pixels carry a solid red 256x256 marker in their top-left corner,
so a decoded result shows where the rotation put it.

Regenerate on macOS with ImageIO:

```bash
swift make_fixture.swift grid_rot90.heic
```

`grid_no_irot.heic` is the same pattern laid out like an older iPhone shot
that carries no `irot` at all, only an EXIF Orientation that points the wrong
way:

- the primary item is a 2048x1536 `grid` of twelve 512x512 `hvc1` tiles, with
  a 320x240 thumbnail, and no `irot` or `imir` property;
- EXIF Orientation 6, which QuickLook ignores on a HEIC, so it displays as
  stored.

ImageIO always writes an `irot`, even for an upright image, so this one comes
from libheif and exiftool instead (macOS, Homebrew `libheif`,
`imagemagick`, `exiftool`):

```bash
magick -size 2048x1536 'xc:rgb(128,128,128)' \
  -fill 'rgb(51,77,204)' -draw 'rectangle 0,1152 2047,1535' \
  -fill red -draw 'rectangle 0,0 255,255' pattern.png
heif-enc -q 30 --cut-tiles 512 -t 320 -o grid_no_irot.heic pattern.png
exiftool -overwrite_original -n -Orientation=6 grid_no_irot.heic
```
