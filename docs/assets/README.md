# Asset kit

This directory contains the original Meld brand assets used by the repository
documentation. They are hand-authored SVG files: no third-party logos are
copied, and no raster preview is fabricated here.

- [logo.svg](logo.svg) — square mark for the README header.
- [social-preview.svg](social-preview.svg) — 1280×640 social preview card. This
  SVG is the authoritative preview asset; use it directly where SVG is
  accepted, or rasterize it externally if a platform requires PNG.

## Palette

| Role | Hex | Use |
| --- | --- | --- |
| Ink | `#102A43` | Dark background and primary text |
| Paper | `#F7F5EF` | Light text and light surfaces |
| Teal | `#0F8B8D` | First language region / primary accent |
| Coral | `#E76F51` | Second language region / bridge accent |
| Gold | `#F2C14E` | Typed-value bridge and highlights |

## Tagline

> One file. Many languages. Typed values.

## Social-preview brief

The preview uses a 2:1 `1280 × 640` canvas so it stays legible in link cards.
The left side carries the Meld mark, wordmark and tagline; the lower strip shows
three language regions joined by a typed-value bridge. The palette and tagline
match the README header and the mdBook stylesheet.

Keep text inside the central safe area when editing, avoid platform logos, and
preserve the high-contrast Paper-on-Ink treatment for small thumbnails.
