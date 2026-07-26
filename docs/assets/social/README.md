# Social card

`og-image.png` is the Open Graph preview for this site, 1200x630, referenced from `website.image` in `../../_quarto.yml`.

It is authored here, in `og-image.typ`, so the card can be regenerated from source.
The template is deliberately local rather than drawn from [quarto-social-cards](https://github.com/mcanouil/quarto-social-cards): that catalogue renders one card per Quarto extension, each carrying the Quarto logo, and `oboro` is a command-line tool.

The mark is redrawn in the template rather than imported from `../icons/icon.svg`.
Typst renders SVG through usvg, which does not evaluate `prefers-color-scheme`, so importing the master would place the light slate disc on this dark background.
Both copies follow the same 32-unit grid: a disc of diameter 26 and a full-width bar of height 6, each centred.

## Fonts

Typst 0.15 cannot read woff2, and the repository ships its typefaces in that format only, so the render decompresses them to TTF first.
This keeps the card reproducible from committed sources rather than from whatever happens to be installed on the machine.

`uvx` runs `fonttools` without installing it as a project dependency.

## Regenerating

Run from the repository root.

```bash
scratch="$(mktemp -d)"

uvx --with brotli --from fonttools fonttools ttLib.woff2 decompress \
  -o "${scratch}/spectral-600.ttf" docs/assets/fonts/spectral-600.woff2
uvx --with brotli --from fonttools fonttools ttLib.woff2 decompress \
  -o "${scratch}/public-sans.ttf" docs/assets/fonts/public-sans-normal.woff2

typst compile --font-path "${scratch}" docs/assets/social/og-image.typ \
  "${scratch}/card.png" --format png --ppi 144

magick "${scratch}/card.png" -resize 1200x630 -alpha off -strip \
  -define png:compression-level=9 docs/assets/social/og-image.png
```

The template's page is 1200pt by 630pt, so `--ppi 144` exports at 2400x1260 and the resize brings it back to the design size, which is what Open Graph consumers expect and what `og:image:width` and `og:image:height` report.

`-alpha off` is lossless here: the card is opaque everywhere, so dropping the constant alpha channel only removes a redundant channel.
