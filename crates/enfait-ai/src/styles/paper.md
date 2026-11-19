# Paper cut-out

## Essence
A paper cut-out animation shot frame by frame: simple shapes, clean edges,
short drop shadows. It looks handmade on a table, with patience and humour.

## Material and rendering
- Each shape is a piece of paper: flat colour, slightly uneven edge (SVG
  path with small bumps, never a perfect circle).
- Short, hard drop shadow under each element (`drop-shadow` 3 to 6 px,
  black at 25 %), the same direction everywhere.
- Background: a coloured paper, discreet fibrous texture, still.

## Colour
Four to five papers at most, saturated but not garish (for example ochre
`#D9A441`, blue `#2F5D8C`, brick red `#C4553A`, cream `#F3E9D2`, ink
`#22201C`). No gradients: paper is plain.

## Typography
Cut-out letters: very bold "Archivo" (weight 800 to 900, narrow width), or
words laid down like labels. Each letter can be a separate piece of paper.

## Motion
- **Animated on twos**: the picture changes 12 times per second. In `seek`,
  round the time: `tl.seek(Math.floor(t * 12) / 12 * 1000)`.
- Shapes slide, rotate and settle; they do not deform and do not scale
  continuously.
- A slight jitter (rotation ±0.6°, offset ±1 px) makes the hand visible. It
  changes every two frames and is computed with
  `enfaitNoise(Math.floor(t * 12) * 7 + shapeIndex)`: never
  `Math.random()` inside `seek`.

## Gestures specific to the style
- A shape enters from the edge of the frame as if pushed by a finger.
- A fold: a shape folds over (`scaleX` towards 0, with a darker face).
- A tear reveals the next scene (transition).

## Pitfalls
- Smooth animation at 30 fps: the craft is lost.
- Shadows in different directions: the table is no longer believable.
- Too many details: paper cut-out lives on simple shapes.

## Freedom
Characters, sets, metaphors, framing: free.
