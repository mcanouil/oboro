# Social card

`og-image.png` is the Open Graph preview for this site, 1200x630, referenced from `website.image` in `../../_quarto.yml`.

It is authored here, in `og-image.typ`, so the card can be regenerated from source.
The template is deliberately local rather than drawn from [quarto-social-cards](https://github.com/mcanouil/quarto-social-cards): that catalogue renders one card per Quarto extension, each carrying the Quarto logo, and `oboro` is a command-line tool.

## The composition

The card is the substitution itself, not a picture with a caption: the real value on the left, the token on the right, and an arrow between them that points both ways, because `oboro restore` puts the value back.
Two rows on one grid, the second smaller and dimmer, which says the substitution is systematic without turning the card into a table.

The chips are `../theme.scss`'s `.oboro-placeholder` at poster scale, down to the soft fill inside the outline, so the card and the site draw one object rather than two drawings of one idea.

Behind it all is a page of monospace text at a few per cent opacity: the "your files" half of the tagline, and the thing the chips are cut from.
It is kept faint enough to be texture; any louder and the eye reads it instead of the substitution, and the card starts to look like it is showing a real document rather than standing for one.

Two things in the template exist because of what Typst does not have.
The wash is faded at the edges by scrims of the background colour running to transparent, since there is no mask.
The double-headed arrow is drawn from two shafts and two triangles rather than set as U+21C6, since none of the three committed typefaces is guaranteed to carry that glyph, and a missing one would rasterise silently.

The mark from `../icons/icon.svg` is not embedded.
Typst renders SVG through usvg, which does not evaluate `prefers-color-scheme`, so importing the master would place the light-scheme colours on this dark card.
It would be redundant in any case: the chips are the mark, at the size where the brackets can hold the real token rather than a slug.

## Fonts

Typst 0.15 cannot read woff2, and the repository ships its typefaces in that format only, so the render decompresses the three the card uses to TTF first.
This keeps the card reproducible from committed sources rather than from whatever happens to be installed on the machine.

`uvx` runs `fonttools` without installing it as a project dependency.

## Regenerating

Run from the repository root.

```bash
scratch="$(mktemp -d)"

for font in spectral-600 public-sans-normal jetbrains-mono-normal; do
  uvx --with brotli --from fonttools fonttools ttLib.woff2 decompress \
    -o "${scratch}/${font}.ttf" "docs/assets/fonts/${font}.woff2"
done

typst compile --font-path "${scratch}" docs/assets/social/og-image.typ \
  "${scratch}/card.png" --format png --ppi 144

magick "${scratch}/card.png" -resize 1200x630 -alpha off -strip \
  -define png:compression-level=9 docs/assets/social/og-image.png
```

The template's page is 1200pt by 630pt, so `--ppi 144` exports at 2400x1260 and the resize brings it back to the design size, which is what Open Graph consumers expect and what `og:image:width` and `og:image:height` report.

`-alpha off` is lossless here: the card is opaque everywhere, so dropping the constant alpha channel only removes a redundant channel.
