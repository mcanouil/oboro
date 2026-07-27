// Oboro social card, 1200x630, rendered to og-image.png.
// See README.md in this directory for the render commands.
//
// The card is the substitution, not a picture with a caption: the real value on
// the left, the token on the right, and an arrow that points both ways because
// `oboro restore` puts the value back. The chips are ../theme.scss's
// `.oboro-placeholder` at poster scale, so the card and the site draw one
// object.
//
// Colours are the _brand.yml palette in its dark pair, since the card is dark.

#let nightfall = rgb("#10161c")
#let moonglow = rgb("#e8edf2")
#let muted = rgb("#9db0c0")
#let haze = rgb("#7fa8c4")
#let moonbright = rgb("#e6c558")

#let inset = 88pt

// The other half of the tagline: a page of somebody's file, set in the site's
// monospace and turned down to almost nothing. Any louder and the eye tries to
// read it instead of the substitution, and the card starts to look like it is
// showing a real document rather than standing for one.
//
// Written as strings rather than as markup: an address such as
// `marie.lefevre@sogexia-partners.fr` would otherwise be parsed as a reference.
#let wash-lines = (
  "Marie Lefevre a rencontre le directeur de Sogexia Partners hier a Lille.",
  "Son numero est le 06 12 34 56 78, et son adresse 12 rue de la Paix, 59000.",
  "Le virement part du compte FR76 3000 6000 0112 3456 7890 189 lundi matin.",
  "Contrat signe par Thomas Vasseur, SIRET 552 100 554 00021, Nord Logistique.",
  "Copie envoyee a marie.lefevre@sogexia-partners.fr et au service juridique.",
  "Le dossier precedent, reference NL-2024-0871, mentionnait Claire Dubourg.",
  "Reunion de suivi prevue le 14 mars, bureau de Roubaix, 3 avenue Jean Lebas.",
  "Facture 2024-119 reglee par carte 4539 1488 0343 6467 le 28 fevrier.",
  "Le prestataire Ouest Conseil, SIREN 803 417 389, intervient sur le lot 2.",
  "Adresse de livraison : 8 boulevard de la Liberte, 59800 Lille, France.",
  "Le contact sur place est Claire Dubourg, joignable au 07 88 45 12 90.",
  "Reglement du solde attendu avant le 31 mars, compte BE71 0961 2345 6769.",
)

// The chip: the moon accent, outlined, on its own soft fill, as the stylesheet
// draws it. `dim` gives the second row the same object with its contrast pulled
// back, since Typst has no element opacity to turn down.
#let chip(body, size: 44pt, dim: false) = box(
  fill: moonbright.transparentize(if dim { 88% } else { 84% }),
  stroke: 2pt + moonbright.transparentize(if dim { 26% } else { 0% }),
  radius: 8pt,
  inset: (x: if dim { 14pt } else { 18pt }, y: if dim { 6pt } else { 8pt }),
  text(
    font: "JetBrains Mono",
    size: size,
    weight: 600,
    fill: moonbright.transparentize(if dim { 20% } else { 0% }),
    body,
  ),
)

// Drawn rather than set: this is the only glyph the card would need beyond the
// Latin text, and none of the three committed typefaces is guaranteed to carry
// U+21C6. Two shafts with opposed heads, in the weight of the type beside them.
#let swap-arrow(size: 44pt) = {
  let width = size * 1.2
  let head = size * 0.28
  let bar = size * 0.08
  let gap = size * 0.32
  box(width: width, height: size, {
    // Upper shaft, pointing right.
    place(left + horizon, dy: -gap / 2, rect(width: width - head, height: bar, fill: moonbright))
    place(
      right + horizon,
      dy: -gap / 2,
      polygon(fill: moonbright, (0pt, -head / 2), (head, 0pt), (0pt, head / 2)),
    )
    // Lower shaft, pointing left.
    place(right + horizon, dy: gap / 2, rect(width: width - head, height: bar, fill: moonbright))
    place(
      left + horizon,
      dy: gap / 2,
      polygon(fill: moonbright, (head, -head / 2), (0pt, 0pt), (head, head / 2)),
    )
  })
}

// Fading the wash at the four edges. Typst has no mask, but the background is a
// flat colour, so a scrim of nightfall running to transparent does the same job
// from the front.
#let scrim(dx, dy, width, height, angle) = place(
  top + left,
  dx: dx,
  dy: dy,
  rect(
    width: width,
    height: height,
    fill: gradient.linear(nightfall, nightfall.transparentize(100%), angle: angle),
  ),
)

#set page(width: 1200pt, height: 630pt, margin: 0pt, fill: nightfall)
#set text(hyphenate: false)

#place(
  top + left,
  dx: -60pt,
  dy: -40pt,
  block(
    width: 1400pt,
    stack(
      spacing: 25pt,
      ..wash-lines.map(line => text(
        font: "JetBrains Mono",
        size: 25pt,
        fill: haze.transparentize(96.5%),
        line,
      )),
    ),
  ),
)

#scrim(0pt, 0pt, 1200pt, 90pt, 90deg)
#scrim(0pt, 540pt, 1200pt, 90pt, 270deg)
#scrim(0pt, 0pt, 110pt, 630pt, 0deg)
#scrim(1090pt, 0pt, 110pt, 630pt, 180deg)

// A single hairline of gold under the wordmark, the width of the word. The card
// carries no other rule, so it reads as a signature rather than as furniture.
#place(
  top + left,
  dx: inset,
  dy: 74pt,
  stack(
    spacing: 26pt,
    text(font: "Spectral", weight: 600, size: 104pt, fill: moonglow, "Oboro"),
    rect(width: 132pt, height: 5pt, fill: moonbright),
  ),
)

// Two rows on one grid, so the arrows line up and the chips start at the same
// x: the second row is the same operation, not a second idea.
#place(
  top + left,
  dx: inset,
  dy: 268pt,
  grid(
    columns: (420pt, 96pt, auto),
    row-gutter: 26pt,
    align: horizon,
    text(font: "JetBrains Mono", size: 44pt, fill: moonglow, "Marie Lefevre"),
    align(center, swap-arrow()),
    chip("[[PERSON_1]]"),

    text(font: "JetBrains Mono", size: 32pt, fill: muted, "06 12 34 56 78"),
    align(center, swap-arrow(size: 32pt)),
    chip("[[PHONE_1]]", size: 32pt, dim: true),
  ),
)

#place(
  top + left,
  dx: inset,
  dy: 486pt,
  block(
    // Breaks after "your", which is the only break in the sentence that does
    // not split a phrase.
    width: 640pt,
    text(
      font: "Public Sans",
      size: 34pt,
      fill: muted,
      "An anonymisation layer between your files and a language model.",
    ),
  ),
)
