// Driver injected by enfait into every page before its own scripts.
// It catches errors, makes randomness deterministic and exposes what is
// needed to move the composition to a precise time and inspect it.
(() => {
  "use strict";
  const errors = [];
  const push = (msg) => {
    if (errors.length < 50) errors.push(String(msg).slice(0, 500));
  };
  window.addEventListener("error", (e) => {
    if (e.target && e.target !== window && (e.target.src || e.target.href)) {
      push("ressource introuvable : " + (e.target.src || e.target.href));
      return;
    }
    const where = e.filename ? ` (${e.filename.split("/").pop()}:${e.lineno})` : "";
    push((e.message || "erreur") + where);
  }, true);
  window.addEventListener("unhandledrejection", (e) => push("unhandled promise rejection: " + e.reason));
  const consoleError = console.error.bind(console);
  console.error = (...args) => {
    push(args.map(String).join(" "));
    consoleError(...args);
  };

  // Reproducible randomness: xorshift32 with a fixed seed.
  let seed = 0x2545f491;
  Math.random = () => {
    seed ^= seed << 13;
    seed ^= seed >>> 17;
    seed ^= seed << 5;
    return (seed >>> 0) / 4294967296;
  };

  const decode = (img) => (img.complete && img.naturalWidth > 0 ? Promise.resolve() : img.decode().catch(() => {}));

  const effectiveOpacity = (el) => {
    let o = 1;
    for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
      const cs = getComputedStyle(n);
      if (cs.display === "none" || cs.visibility === "hidden") return 0;
      o *= parseFloat(cs.opacity);
    }
    return o;
  };

  const ownText = (el) =>
    Array.from(el.childNodes)
      .filter((n) => n.nodeType === 3)
      .map((n) => n.textContent)
      .join("")
      .trim();

  // Deterministic noise in [-1, 1] for an integer: same input, same
  // output, whatever the order of the calls.
  window.enfaitNoise = (n) => {
    let x = (Math.imul(n | 0, 0x27d4eb2d) ^ 0x165667b1) >>> 0;
    x = Math.imul(x ^ (x >>> 15), 0x85ebca6b) >>> 0;
    x = Math.imul(x ^ (x >>> 13), 0xc2b2ae35) >>> 0;
    return ((x ^ (x >>> 16)) >>> 0) / 2147483647.5 - 1;
  };

  window.__enfait = {
    errors,

    // Waits for fonts and images, checks the contract.
    async ready() {
      await document.fonts.ready;
      await Promise.all(Array.from(document.images).map(decode));
      const api = window.enfait;
      if (!api || typeof api.seek !== "function") {
        return { ok: false, duration: 0, errors, reason: "window.enfait.seek absent" };
      }
      const duration = Number(api.duration);
      if (!Number.isFinite(duration) || duration <= 0) {
        return { ok: false, duration: 0, errors, reason: "window.enfait.duration invalide" };
      }
      // Going to the end then back to the start applies the initial state of
      // each animation (anime.js only does it when passing over it).
      await api.seek(duration);
      await api.seek(0);
      return { ok: true, duration, errors, reason: null };
    },

    // Moves the composition to `t` seconds. `scenes` maps a scene name
    // (`from`, `to`) to the URL of the image to show for this frame.
    async seek(t, scenes) {
      if (scenes) {
        const pending = [];
        for (const [name, src] of Object.entries(scenes)) {
          for (const img of document.querySelectorAll(`img[data-enfait-scene="${name}"]`)) {
            if (img.getAttribute("src") !== src) {
              img.setAttribute("src", src);
              pending.push(decode(img));
            }
          }
        }
        await Promise.all(pending);
      }
      await window.enfait.seek(t);
      for (const anim of document.getAnimations()) {
        anim.pause();
        anim.currentTime = t * 1000;
      }
      for (const video of document.querySelectorAll("video")) {
        video.pause();
        video.currentTime = t;
      }
      // The frame is then produced by beginFrame: no wait here.
      return true;
    },

    // Problems visible at the current time: text outside the safe area,
    // unknown fonts.
    inspect(safeMargin, knownFamilies) {
      const W = innerWidth;
      const H = innerHeight;
      const mx = W * safeMargin;
      const my = H * safeMargin;
      const known = new Set(knownFamilies.map((f) => f.toLowerCase()));
      const generic = new Set(["serif", "sans-serif", "monospace", "cursive", "system-ui", "inherit"]);
      const outside = [];
      const fonts = new Set();
      for (const el of document.body.querySelectorAll("*")) {
        const text = ownText(el);
        if (!text) continue;
        if (effectiveOpacity(el) < 0.05) continue;
        const family = getComputedStyle(el).fontFamily.split(",")[0].trim().replace(/^["']|["']$/g, "");
        if (!known.has(family.toLowerCase()) && !generic.has(family.toLowerCase())) fonts.add(family);
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0) continue;
        if (r.right <= 0 || r.bottom <= 0 || r.left >= W || r.top >= H) continue; // off screen: intended
        if (r.left < mx - 1 || r.top < my - 1 || r.right > W - mx + 1 || r.bottom > H - my + 1) {
          outside.push({ text: text.slice(0, 60), left: r.left, top: r.top, right: r.right, bottom: r.bottom });
        }
      }
      return { outside: outside.slice(0, 10), unknownFonts: Array.from(fonts) };
    },
  };
})();
