# Swiss typography

## Essence
The International Typographic Style: strict grid, asymmetry, text as the
only material. Information is beautiful because it is ordered. Rigour,
contrast, no decoration.

## Material and rendering
- A 12-column grid visible in the decisions, never drawn.
- Pure flat fills, thin rules (2 px) and big numbers. No texture, no
  shadow, no figurative illustration: primary geometric shapes if needed.

## Colour
Off-white background `#F4F2EE` or black `#111111`. One strong colour: red
`#E63027`. The rest in black and grey.

## Typography
"Inter" only, contrasting weights (400 and 800). Flush left, ragged right.
Very large sizes for a word or a number (300 px and more), tight leading,
negative tracking. Secondary texts are small and numerous, like captions.

## Motion
- Elements slide along the grid: horizontal or vertical moves only, never
  diagonal.
- Reveals by rectangular mask (`clipPath: inset(...)`).
- Sharp easings: `inOutExpo`, `outQuart`. Clean stops.
- Layout changes rearrange the same blocks, they do not replace them.

## Gestures specific to the style
- A giant number counts up (counter) while its caption writes itself
  beside it.
- A whole column shifts by one module and pushes the others.
- The frame is split in two by a rule crossing the screen (transition).

## Pitfalls
- Centring text: the very opposite of the style.
- Adding icons or illustrations: text is enough.
- Soft, floating moves: the style is mechanical and precise.

## Freedom
Hierarchy, highlighted words, rhythm of the grid: free.
