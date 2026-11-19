# Archive engraving

## Essence
A 19th-century dictionary plate coming to life. Everything is line work:
hatching, intaglio, framing rules. Knowledge looks old, precise and a little
solemn. Not an "old paper effect": a real printing logic.

## Material and rendering
- Rag paper background, around `#EFE8D8`, slightly uneven: a very soft
  grain computed once in a canvas, still.
- Shapes built from parallel or crossed hatching (SVG `pattern`s of lines),
  never flat fills. Volume comes from the density of the lines, not from a
  gradient.
- Ink `#1E1B17` at full opacity. Greys are hatching, not transparency.
- Double rules for framing, drop caps, plate numbers ("Pl. III").

## Colour
A single accent colour, laid down like a stamp or sealing wax: vermilion
`#B23A2A`. It points to a single thing per scene.

## Typography
Titles in "Fraunces" (high opsz, weight 500 to 700), captions in italic
"Instrument Serif". Spaced small caps for numbers and dates. Never a
sans-serif.

## Motion
- Lines draw themselves (`svg.createDrawable`), at an engraver's pace: fast,
  regular, left to right.
- Hatching arrives in successive layers, like passes of the burin.
- No bounce, no elasticity. Easings `outCubic`, `inOutQuad`.
- The camera glides slowly over the plate, like an eye reading.

## Gestures specific to the style
- A numbered reference ("fig. 2") links a detail to its caption with a rule
  drawing itself.
- An oval medallion is cut out of the plate to isolate a portrait.
- A page turns, revealing the next plate (transition).

## Pitfalls
- Hatching too regular: it looks like a grid or a barcode. Vary the spacing
  with the shape.
- Animated paper texture: it flickers. It stays still.
- Sepia everywhere: that is a filter, not an engraving.

## Freedom
Composition, subject of the plates, number of figures, place of the text:
free.
