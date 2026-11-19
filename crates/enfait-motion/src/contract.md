# Technical contract of an enfait composition

A composition is **one standalone HTML document** that animates a scene of
fixed duration. It is rendered frame by frame by a headless Chromium, then
encoded as video. Nothing plays in real time.

## What the page must declare

```js
window.enfait = {
  duration: 4.2,                       // seconds, equal to the requested duration
  seek(t) { tl.seek(t * 1000); },      // t in seconds; anime.js counts in ms
};
```

- `seek(t)` must be **pure**: the same `t` always gives the same frame,
  whatever the order of the calls (the engine may go back in time).
- The simplest: a single anime.js timeline **without autoplay**
  (`createTimeline({ autoplay: false })`), built on load, and `seek(t)`
  calling `tl.seek(t * 1000)`.
- No `setTimeout`, `setInterval`, `requestAnimationFrame`, `Date.now()`
  nor any `animate(...)` playing on its own: only `t` moves the scene.
- `Math.random()` is deterministic at load time, but **not inside `seek`**
  (the result would depend on the order of the calls). For time-dependent
  noise, use `enfaitNoise(n)`, which always returns the same value in
  [-1, 1] for the same integer `n` (for example
  `enfaitNoise(Math.floor(t * 12))`).

## Environment

- Page size: the one given in the request (for example 1920×1080). Size
  things in `px` on that basis. No scrolling (`overflow: hidden` on
  `html, body`).
- Already loaded for you, **do not include them**:
  - anime.js 4 (global `anime`);
  - the fonts listed in the request (`font-family: "Inter"` etc.).
- **No network access**: no CDN, no Google Fonts, no remote image. Draw in
  inline HTML/CSS/SVG.
- No audio nor `<video>` (the user's videos are used for b-roll shots,
  edited separately).
- **Textures** (grain, paper, noise): compute them once at load time (in a
  `<canvas>` turned into a background image, or a small repeated pattern),
  then leave them still. No SVG `feTurbulence` or `feDisplacementMap`
  filter over a large surface: it is recomputed on every frame and slows
  the render down.

## anime.js 4: the essentials

This is **version 4**, whose API differs from v3 (no `anime({...})`, no
`anime.timeline`, no `targets:` in the parameters).

```js
const { createTimeline, stagger, splitText, svg, utils } = anime;

const tl = createTimeline({ autoplay: false, defaults: { ease: "outExpo", duration: 600 } });

// tl.add(targets, properties, position)
tl.add(".title", { y: ["40px", "0px"], opacity: [0, 1] }, 0)       // at 0 ms
  .add(".line", { scaleX: [0, 1], ease: "inOutExpo" }, 350)        // at 350 ms
  .add(".tag", { opacity: [0, 1] }, "-=200")                       // 200 ms before the end of the previous one
  .add(".bg", { scale: [1, 1.03], ease: "linear", duration: 4200 }, 0);
tl.set(".hidden", { opacity: 0 }, 2000);                           // instant change
```

- **Time in milliseconds.** Position: number (absolute ms from the start of
  the timeline), `"+=200"` / `"-=200"` (relative to the end of the previous
  addition), `"<"` (end of the previous one), `"<<"` (start of the previous
  one). Prefer absolute positions set on the times of the words.
- **Properties**: transform shorthands `x`, `y`, `scale`, `scaleX`,
  `rotate`, `skewX`; CSS in camelCase (`opacity`, `clipPath`,
  `backgroundColor`, `letterSpacing`); CSS variables (`"--p"`); SVG
  attributes (`r`, `cx`, `strokeWidth`). Start and end value:
  `[from, to]`. Units included: `"110%"`, `"2rem"`, `"-4deg"`.
- **Easings** (`ease`): `"linear"`, `"inQuad"`, `"outQuad"`, `"inOutQuad"`,
  `"outCubic"`, `"outQuart"`, `"outExpo"`, `"inOutExpo"`, `"inBack"`,
  `"outBack(1.4)"`, `"outElastic(1, .6)"`, `"out(3)"`, `"inOut(4)"`,
  `"steps(6)"`. No GSAP names (`power3.out` does not exist here).
- **Staggers**: `delay: stagger(40)`, `stagger(60, { from: "center" })`,
  `stagger(50, { start: 300 })`, `stagger([0, 400])` (spread over 400 ms).
- **Keyframes**: `y: [{ to: -30, duration: 250 }, { to: 0, ease: "outBounce" }]`.
- **Split text**:
  ```js
  const { lines, words, chars } = splitText(".title", {
    lines: true, words: true, chars: { wrap: "clip" },  // "clip": mask to reveal
  });
  tl.add(chars, { y: ["110%", "0%"], delay: stagger(30) }, 200);
  ```
- **SVG paths**: `tl.add(svg.createDrawable("#path"), { draw: ["0 0", "0 1"], duration: 1200 }, 0)`.
- **Morphing**: `tl.add("#shape", { d: svg.morphTo("#target") }, 800)` (or
  `points` for a polygon).
- **Motion path**:
  `tl.add(".dot", { ease: "linear", duration: 2000, ...svg.createMotionPath("#route") }, 0)`.
- **Counters**: animate an object and write the text in `onUpdate`, which is
  also called during `seek`:
  ```js
  const n = { v: 0 };
  tl.add(n, { v: 1250, duration: 1200, modifier: utils.round(0),
    onUpdate: () => { el.textContent = String(n.v).replace(/\B(?=(\d{3})+(?!\d))/g, " "); } }, 300);
  ```
  Do not use `toLocaleString` (invisible separators in some fonts).
- Animated elements must exist when the timeline is built. Avoid
  `onComplete`, `onBegin` and any logic depending on the playing order.

## Frames provided by the engine (transitions, scenes with the take)

A transition composition receives the two shots it connects, and a
"mixed" or "effect" scene receives the filmed take (the same frame in
`from` and `to`), as images updated on every frame by the engine:

```html
<img data-enfait-scene="from" style="position:absolute; inset:0; width:100%; height:100%">
<img data-enfait-scene="to"   style="position:absolute; inset:0; width:100%; height:100%">
```

Do not set their `src`. Animate them as layers (masks, `clipPath`,
transforms, opacity). For a transition, at `t = 0` only `from` must be
visible, at `t = duration` only `to`. For a scene with the take, only use
`from` (hide `to`) and do not distort the filmed person, unless the request
says that `to` holds the cut-out person: it is then a transparent layer
aligned on `from`, to put above your elements.

## Expected render

- The page background is opaque (set `background` on `html, body`).
- Important text stays within the **safe area** (5 % margin on each edge)
  while it is readable.
- The complete document fits in a single ```` ```html ```` block.
