// Oboro social card, 1200x630, rendered to og-image.png.
// See README.md in this directory for the render commands.
//
// Colours are the _brand.yml palette. The card is dark, so the mark uses the
// dark pair: `haze` disc, `moonbright` bar.

#let nightfall = rgb("#10161c")
#let moonglow = rgb("#e8edf2")
#let haze = rgb("#7fa8c4")
#let moonbright = rgb("#e6c558")
#let muted = rgb("#9db0c0")

// The mark from ../icons/icon.svg, redrawn rather than imported: Typst renders
// SVG through usvg, which ignores `prefers-color-scheme` just as librsvg does,
// so `image("../icons/icon.svg")` would put the light slate disc on this dark
// background. Two primitives is cheaper than a third copy of the artwork.
// Proportions are the SVG's 32-unit grid: a disc of diameter 26 and a
// full-width bar of height 6, both centred.
#let mark(size) = box(width: size, height: size, {
  place(center + horizon, circle(radius: size * 13 / 32, fill: haze, stroke: none))
  place(center + horizon, rect(width: size, height: size * 6 / 32, fill: moonbright, stroke: none))
})

#set page(width: 1200pt, height: 630pt, margin: 88pt, fill: nightfall)
#set text(hyphenate: false)

#align(
  horizon,
  grid(
    columns: (200pt, 1fr),
    column-gutter: 72pt,
    align: horizon,
    mark(200pt),
    stack(
      spacing: 32pt,
      text(font: "Spectral", weight: 600, size: 120pt, fill: moonglow, "Oboro"),
      text(
        font: "Public Sans",
        size: 40pt,
        fill: muted,
        "An anonymisation layer between your files and a language model.",
      ),
    ),
  ),
)
