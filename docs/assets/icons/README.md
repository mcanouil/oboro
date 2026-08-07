# Icons

Everything here derives from `icon.svg`, the hand-authored master: a bracketed placeholder, which is what the tool makes.
Nothing is traced, upscaled, or model-generated. So the whole set is reproducible from source.

The mark is the same object `../theme.scss` draws around every `[[PERSON_1]]` on the site, in its `.oboro-placeholder` rule.
The brackets are oboro's marking. The slug is the value they stand in for.
It is deliberately not a redaction bar. The tool substitutes reversibly: `oboro restore` puts the real values back, and the document still reads as a document.

Three flat shapes on a 32-unit grid, and flat on purpose.
Each bracket is a stem with two arms. The arms keep it from reading as a battery or a pause glyph, so they run the full width of the counter.
The slug spans arm tip to arm tip, with two units of air above and below. Narrower, and it reads as a cursor. Taller, and it collides with the arms at 16x16.

The colours come from `../../_brand.yml`: `moon` and `slate` in the light scheme, `moonbright` and `haze` in the dark one.
`icon.svg` carries an inline `@media (prefers-color-scheme: dark)` rule. So browsers that load it directly flip it with the theme.
The light values are presentation attributes on the shapes. So a renderer that ignores CSS still gets a valid icon.

The card in `../social/` is a separate drawing rather than this file scaled up, for reasons given in its own README.

## Rasters

The rasters sit on the `#1b242e` navbar plate, which is dark in both colour schemes.
librsvg does not evaluate `prefers-color-scheme`, so `raster-dark.css` forces the dark variant. Without it, the slug and the darker gold would lose most of their contrast against that plate.
Its values must stay in step with the `@media` block in `icon.svg`: they are the same pair, applied by a different route.
If that file ever stops matching a selector, the light and dark renders come out identical. That is the failure to check for.

Tools: `rsvg-convert` (librsvg) for the vector to raster step, `magick` (ImageMagick 7) for padding, flattening, and `.ico` assembly.
Neither is a build dependency. Run the commands by hand when the master changes.

Run from this directory.

```bash
for size in 32 144 154 410; do
  rsvg-convert -s raster-dark.css -w "${size}" -h "${size}" icon.svg -o "/tmp/icon-${size}.png"
done

magick /tmp/icon-32.png -background '#1b242e' -flatten -strip /tmp/icon-32-flat.png
magick /tmp/icon-32-flat.png -define icon:format=png ../../favicon.ico
magick /tmp/icon-144.png -background '#1b242e' -gravity center -extent 180x180 -flatten apple-touch-icon.png
magick /tmp/icon-154.png -background '#1b242e' -gravity center -extent 192x192 -flatten icon-192.png
magick /tmp/icon-410.png -background '#1b242e' -gravity center -extent 512x512 -flatten icon-512.png
```

The intermediate sizes give the padded icons roughly 10% margin.

Three flat colours, and almost no antialiasing, mean every output palettes on its own: the favicon is 766 bytes at 4 bits, and none of the PNG files reaches a kilobyte.
The favicon is still flattened to an opaque PNG first. Writing the `.ico` from a transparent render makes ImageMagick store an uncompressed 32-bit bitmap, whatever the colour count.

| File                   | Size    | Purpose                                                    |
| ---------------------- | ------- | ---------------------------------------------------------- |
| `icon.svg`             | vector  | `extensions.atelier.icon`, and the source for the rest     |
| `../../favicon.ico`    | 32x32   | `website.favicon`, for clients that ignore the SVG         |
| `apple-touch-icon.png` | 180x180 | `extensions.atelier.apple-touch-icon`, opaque, 10% padding |
| `icon-192.png`         | 192x192 | `../../site.webmanifest`                                   |
| `icon-512.png`         | 512x512 | `../../site.webmanifest`                                   |

There is no maskable icon: this is a documentation site, not an installable application.
