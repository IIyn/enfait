# enfait's motion design guide

These rules apply to every video. The user's style rules, when there are
any, come first.

## What we make

Animated scenes accompanying a voice-over: YouTube videos, tutorials,
explainers, product presentations. The picture **shows** what the voice
**says**, when it says it. The viewer is not reading a slideshow: they
listen, and the picture makes the idea obvious.

## One scene, one idea

- A single focal point at a time. If two things move, one leads, the other
  follows.
- Never write the whole sentence on screen: subtitles are a separate layer.
  On screen, only key words, numbers, names. Six words at most visible at
  the same time, unless explicitly asked.
- Prefer the concrete to the abstract: a diagram being built, a chart
  growing, a drawn interface, a counter, a map, an SVG object. A simple
  visual metaphor is better than a title.

## Time

- **Sync with the voice.** An element tied to a word appears on that word,
  arriving 2 to 4 frames before it (about 0.1 s) to be in place when it is
  heard. The times of the words are given: use them.
- Entrances: 0.35 to 0.7 s. Exits: 0.25 to 0.4 s. Important text stays
  readable at least 0.8 s.
- Nothing is completely still: a slow drift (scale 1 → 1.03, or a slight
  move) keeps the scene alive between two events.
- The scene is settled at its end: no animation cut mid-flight at the block
  change, unless the direction plans a continuity. Do not empty the screen
  at the end: the edit handles the passage to the next block.
- The rhythm varies: a dense scene alternates with a scene that breathes.
  Within a scene too, fast moments and settled ones.
- No dead time of more than one second without a visual event, and never
  an empty screen at the start: something is visible from 0.2 s.
- At the end of the scene, all visible text is complete and readable (no
  scrambling or typing in progress).

## Motion

- Easings: `outCubic` / `outExpo` for entrances, `inQuad` for exits,
  `inOutCubic` / `inOutExpo` for moves. Linear only for continuous motion
  (slow rotation, scrolling). An overshoot (`outBack(1.4)`) from time to
  time, not everywhere.
- Staggers between 20 and 80 ms for letters, 60 to 150 ms for words or list
  items.
- Reveals by mask rather than by fade: letters or lines rising behind a
  mask (`splitText` with `wrap: "clip"`), `clipPath`, paths drawing
  themselves (`svg.createDrawable`), shapes morphing (`svg.morphTo`).
- Animate `transform` and `opacity` preferably.
- The camera can move: a global container zooming or sliding to the next
  element flows better than a screen emptying.

## Composition

- Use the whole frame. Asymmetric layouts, large masses, assumed empty
  space. Align on a grid (5 % margins minimum).
- Clear hierarchy: three text sizes at most. At 1080p: 120 to 220 px for a
  key word, 56 to 90 px for a title, 36 px minimum for secondary text.
- Two font families at most: one for display, one for text. Tight tracking
  (negative `letter-spacing`) on large titles, tabular figures for
  counters.
- Colours: those of the art direction. The accent goes to the most
  important element of the scene, not to decoration.
- Icons and illustrations are drawn in inline SVG, strokes of equal width,
  rounded caps, consistent from one scene to the next.

## What we do not do

These tics give a generic, "made by an AI" look:

- purple-blue gradients, glowing halos, neon, frosted glass, coloured
  blurred shadows;
- particles or sparks floating for no reason, decorative bubbles;
- emojis, clip art, placeholder text, fake logos;
- pill-shaped everything, cards stacked everywhere;
- text bouncing for no reason, gratuitous 3D rotations;
- a whole centred sentence fading in.

## Before rendering

- Check the key times mentally: at `t = 0`, at the first key word, in the
  middle, at the end. Each frame must be a good still image.
- Readable text stays within the safe area.
- The declared duration is exactly the requested one.
