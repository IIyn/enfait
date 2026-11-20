(function () {
  'use strict';
  const TEXT = window.EnfaitText;
  const SCENES = window.EnfaitScenes;
  const HOLD = 7000; // time on each style before moving to the next one
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* Language --------------------------------------------------------- */
  function store(k, v) {
    try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, v); } catch (_) { return null; }
    return null;
  }
  const fromUrl = new URLSearchParams(location.search).get('lang');
  let lang = fromUrl || store('enfait.lang') || ((navigator.language || '').toLowerCase().startsWith('fr') ? 'fr' : 'en');
  if (!TEXT[lang]) lang = 'en';

  function t(key) { return TEXT[lang][key] ?? TEXT.fr[key] ?? key; }

  function applyLang() {
    document.documentElement.lang = lang;
    document.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = t(el.dataset.i18n); });
    document.querySelectorAll('[data-i18n-html]').forEach((el) => { el.innerHTML = t(el.dataset.i18nHtml); });
    document.querySelectorAll('[data-i18n-attr]').forEach((el) => {
      const [attr, key] = el.dataset.i18nAttr.split(':');
      el.setAttribute(attr, t(key));
    });
    document.querySelectorAll('.lang button').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.lang === lang)));
    updatePauseLabel();
  }

  document.querySelectorAll('.lang button').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.lang === lang) return;
    lang = b.dataset.lang;
    store('enfait.lang', lang);
    applyLang();
    show(current, true);
  }));

  /* Stage scaling ---------------------------------------------------- */
  const stage = document.querySelector('.stage');
  const frame = stage.querySelector('.frame');
  new ResizeObserver(() => {
    frame.style.transform = `scale(${stage.clientWidth / 1280})`;
  }).observe(stage);

  /* Wish typing ------------------------------------------------------ */
  const wish = document.querySelector('.ask-wish');
  const wishText = wish.querySelector('.wish-text');
  let typing = 0;
  function typeWish(text, style) {
    clearInterval(typing);
    wish.dataset.style = style;
    wishText.textContent = '';
    if (reduced) { wishText.textContent = text; return; }
    let i = 0;
    typing = setInterval(() => {
      if (i >= text.length) { clearInterval(typing); return; }
      i += 1;
      // Cut-paper letters each get their own slight tilt.
      if (style === '4') {
        const ch = text[i - 1];
        let word = wishText.lastElementChild;
        if (ch === ' ' || !word) {
          if (ch === ' ') wishText.appendChild(document.createTextNode(' '));
          word = document.createElement('span');
          word.className = 'w';
          wishText.appendChild(word);
          if (ch === ' ') return;
        }
        const s = document.createElement('span');
        s.textContent = ch;
        s.style.rotate = `${((i * 37) % 9) - 4}deg`;
        word.appendChild(s);
      } else {
        wishText.textContent = text.slice(0, i);
      }
    }, 26);
  }

  /* Rotation --------------------------------------------------------- */
  const tabs = [...document.querySelectorAll('.styles button')];
  const credit = document.querySelector('.credit');
  const hero = document.querySelector('.hero');
  const pauseBtn = document.querySelector('.pause');
  let current = 0;
  let stopScene = null;
  let paused = reduced;
  let hovered = false;

  function show(i, force) {
    if (i === current && !force && stopScene) return;
    current = i;
    if (stopScene) stopScene();
    frame.innerHTML = '';
    stage.dataset.scene = String(i);
    stopScene = SCENES[i](frame, lang);
    typeWish(t('w.' + i), String(i));
    credit.textContent = t(i === 0 ? 'hero.credit1' : 'hero.creditN');
    tabs.forEach((b, k) => {
      b.classList.toggle('is-active', k === i);
      b.setAttribute('aria-current', k === i ? 'true' : 'false');
      const bar = b.querySelector('.bar i');
      bar.style.animation = 'none';
      void bar.offsetWidth; // restart the CSS animation
      bar.style.animation = '';
    });
  }

  tabs.forEach((b) => {
    b.querySelector('.bar i').addEventListener('animationend', () => {
      if (b.classList.contains('is-active')) show((current + 1) % SCENES.length);
    });
    b.addEventListener('click', () => show(Number(b.dataset.i), true));
  });

  function syncPause() {
    hero.classList.toggle('is-paused', paused || hovered || document.hidden);
    pauseBtn.setAttribute('aria-pressed', String(paused));
    updatePauseLabel();
  }
  function updatePauseLabel() {
    if (pauseBtn) pauseBtn.setAttribute('aria-label', t(paused ? 'a11y.play' : 'a11y.pause'));
  }
  pauseBtn.addEventListener('click', () => { paused = !paused; syncPause(); });
  stage.addEventListener('pointerenter', () => { hovered = true; syncPause(); });
  stage.addEventListener('pointerleave', () => { hovered = false; syncPause(); });
  document.addEventListener('visibilitychange', syncPause);
  document.documentElement.style.setProperty('--hold', HOLD + 'ms');

  /* Steps track: waveform and scroll playhead ------------------------ */
  const track = document.querySelector('.track');
  const wave = track.querySelector('.wave');
  const head = track.querySelector('.playhead');
  (function drawWave() {
    const n = 180;
    let s = 3;
    const r = () => ((s = (s * 16807) % 2147483647) / 2147483647);
    let bars = '';
    for (let i = 0; i < n; i++) {
      const env = 0.35 + 0.65 * Math.abs(Math.sin(i / 9) * Math.cos(i / 23));
      const h = Math.max(0.08, env * (0.4 + r() * 0.6));
      bars += `<rect x="${i * 2 + 0.3}" y="${(1 - h) * 10}" width="1.2" height="${h * 20}" rx=".6"/>`;
    }
    wave.setAttribute('viewBox', `0 0 ${n * 2} 20`);
    wave.innerHTML = bars;
  })();
  const waveBars = wave.querySelectorAll('rect');
  function onScroll() {
    const r = track.getBoundingClientRect();
    const vh = innerHeight;
    const p = Math.min(1, Math.max(0, (vh * 0.75 - r.top) / (r.height + vh * 0.35)));
    head.style.setProperty('--p', p.toFixed(4));
    const lit = Math.round(p * waveBars.length);
    waveBars.forEach((b, i) => b.classList.toggle('lit', i < lit));
  }
  addEventListener('scroll', onScroll, { passive: true });
  addEventListener('resize', onScroll);

  /* Latest release links -------------------------------------------- */
  const REPO = 'IIyn/enfait';
  const links = document.querySelectorAll('[data-asset]');
  links.forEach((a) => { a.href = `https://github.com/${REPO}/releases/latest`; });
  fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: 'application/vnd.github+json' } })
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((rel) => {
      const v = String(rel.tag_name || '').replace(/^v/, '');
      if (!v) return;
      const names = new Set((rel.assets || []).map((x) => x.name));
      links.forEach((a) => {
        const name = a.dataset.asset.replace('{v}', v);
        a.textContent = name;
        if (names.has(name)) a.href = `https://github.com/${REPO}/releases/download/${rel.tag_name}/${name}`;
      });
      document.querySelector('.ver').textContent = rel.tag_name;
    })
    .catch(() => {});

  applyLang();
  syncPause();
  const first = Number(new URLSearchParams(location.search).get('s')) || 0;
  show(Math.min(SCENES.length - 1, Math.max(0, first)), true);
  onScroll();
})();
