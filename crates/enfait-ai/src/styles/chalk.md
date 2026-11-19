# Chalkboard

## Essence
A passionate teacher explains at the board. We watch the idea being built
stroke by stroke, with arrows, boxes and crossings-out. It is lively,
imperfect and clear.

## Material and rendering
- Slate background `#2B3A33`, with still, very discreet erasing marks.
- Chalk strokes: SVG outlines with a slightly irregular path (small
  variations computed once on the path points), a still grain pattern laid
  as a mask, and a slight uneven transparency. Never a solid fill: fill
  with quick hatching.
- Constant thickness of about 5 px, rounded caps.

## Colour
White chalk `#ECEBE4` for most things. Two coloured chalks at most: yellow
`#F2D46B` to underline, pink `#E8908E` to correct or contrast.

## Typography
"Caveat" for everything handwritten, weight 500 to 700. Words write
themselves letter by letter (`splitText` then a staggered appearance of 40
to 70 ms). Titles are underlined by hand.

## Motion
- Everything draws itself in the order a hand would (`svg.createDrawable`),
  at writing speed.
- Arrows start from an existing element towards the new one.
- The camera reframes the board to follow the hand, with short pans.

## Gestures specific to the style
- Circling a key word with a stroke that does not quite close.
- Crossing out a wrong idea, then writing the right one above.
- A sweep of the sponge wipes the board (transition).

## Pitfalls
- Perfectly regular strokes: it looks like vector art.
- Writing everything at once: chalk is sequential.
- Too much text: a board is key words and diagrams.

## Freedom
Diagrams, building order, placement of the elements: free.
