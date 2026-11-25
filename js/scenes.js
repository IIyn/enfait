// The five hero renders. Each scene draws into a fixed 1280×720 frame that
// the stage scales to its width, and returns a function that stops it.
(function () {
  'use strict';
  const { animate, createTimeline, stagger, utils, svg, steps } = anime;
  const NS = 'http://www.w3.org/2000/svg';
  const W = 1280;
  const H = 720;

  function html(root, markup) {
    root.innerHTML = markup;
    return (sel) => root.querySelector(sel);
  }

  // Collects everything a scene starts so it can be stopped in one call.
  function bag() {
    const anims = [];
    const timers = [];
    const frames = [];
    return {
      a(x) { anims.push(x); return x; },
      every(ms, fn) { timers.push(setInterval(fn, ms)); },
      later(ms, fn) { timers.push(setTimeout(fn, ms)); },
      raf(fn) {
        const loop = (t) => { fn(t); frames[0] = requestAnimationFrame(loop); };
        frames[0] = requestAnimationFrame(loop);
      },
      stop() {
        anims.forEach((x) => { try { x.revert(); } catch (_) { x.pause && x.pause(); } });
        timers.forEach((t) => { clearInterval(t); clearTimeout(t); });
        frames.forEach(cancelAnimationFrame);
      },
    };
  }

  /* 1. Facecam: pause, a pointer cuts the creator out, subscribe. ------ */
  // Outline of the creator at the pause frame (1.6 s), traced from the
  // cut-out in media/facecam-cut.webp. Left open along the frame border.
  const S1_OUTLINE = 'M1202 719 L1214 700 L1223 672 L1229 667 L1252 660 L1258 653 L1263 636 L1263 591 L1257 562 L1251 547 L1215 511 L1201 478 L1162 419 L1144 395 L1086 337 L1068 324 L1049 302 L999 269 L935 246 L912 243 L870 230 L837 206 L850 189 L862 152 L861 133 L850 125 L854 89 L858 77 L858 54 L854 40 L845 24 L824 0 M670 0 L630 37 L627 44 L627 59 L633 73 L637 112 L645 133 L645 172 L648 195 L663 250 L612 277 L582 284 L568 291 L521 353 L510 384 L488 405 L480 409 L476 407 L472 411 L462 412 L456 417 L456 421 L452 425 L440 428 L436 433 L426 430 L405 455 L392 466 L377 473 L348 480 L338 487 L339 538 L344 562 L344 588 L339 595 L329 601 L317 613 L313 624 L320 642 L321 655 L333 698 L342 719';
  const S1_PAUSE = 1.6;
  // Point the camera pushes in towards after the cut-out (the face).
  const S1_FOCUS = [745, 165];

  function footage(root, lang) {
    const b = bag();
    const fr = lang === 'fr';
    // State classes live on the frame, which outlives the scene: start clean
    // and clear them on stop, or a rerun (language switch, next loop) would
    // open already cut out and subscribed.
    const states = ['is-frozen', 'is-cut', 'is-sub'];
    root.classList.remove(...states);
    // Click lands on the chest (frame coordinates).
    const cx = 770;
    const cy = 470;
    const $ = html(root, `
      <div class="s1-shot" style="transform-origin: ${S1_FOCUS[0]}px ${S1_FOCUS[1]}px">
      <video class="s1-video" src="media/facecam.mp4" poster="media/facecam.jpg" muted playsinline preload="auto"></video>
      <img class="s1-still" src="media/facecam-pause.jpg" alt="">
      <div class="s1-dim"></div>
      <img class="s1-cut" src="media/facecam-cut.webp" alt="">
      <svg class="s1-fx" viewBox="0 0 ${W} ${H}">
        <path class="s1-glow" d="${S1_OUTLINE}"/>
        <path class="s1-line" d="${S1_OUTLINE}"/>
        <circle class="s1-ring" cx="${cx}" cy="${cy}" r="14"/>
      </svg>
      </div>
      <div class="s1-paused"><i></i><i></i></div>
      <div class="s1-sub">
        <span class="s1-avatar"></span>
        <span class="s1-chan"><b>enfait</b><small>${fr ? '12,4 k abonnés' : '12.4K subscribers'}</small></span>
        <span class="s1-btn"><span class="s1-label">${fr ? 'S’abonner' : 'Subscribe'}</span><span class="s1-done">${fr ? 'Abonné' : 'Subscribed'}</span></span>
        <svg class="s1-bell" viewBox="0 0 24 24" width="40" height="40"><path d="M12 3a6 6 0 0 0-6 6v4l-2 3h16l-2-3V9a6 6 0 0 0-6-6zm-2.5 15a2.5 2.5 0 0 0 5 0z"/></svg>
      </div>
      <div class="s1-cursor"><svg viewBox="0 0 24 24" width="54" height="54">
        <path d="M4 2.5v17.2l4.3-4.1 3 6.7 3.1-1.4-3-6.6h6z" fill="#fff" stroke="#0E1110" stroke-width="1.5" stroke-linejoin="round"/>
      </svg></div>`);
    const video = $('.s1-video');
    video.currentTime = 0;
    video.play().catch(() => {});

    const cursor = $('.s1-cursor');
    const arrow = cursor.firstElementChild;
    // Tip of the arrow is at (4, 2.5) in a 24 unit box drawn at 54 px.
    const tip = (x, y) => ({ left: x - 9, top: y - 5.6 });
    const outline = svg.createDrawable(root.querySelectorAll('.s1-line, .s1-glow'));
    utils.set(outline, { draw: '0 0' });

    // Freeze on the exact frame the cut-out was taken from: a still of it
    // covers the video, since a seek can land one frame off and the camera
    // moves, which would show a ghost behind the cut-out.
    b.later(S1_PAUSE * 1000, () => {
      video.pause();
      root.classList.add('is-frozen');
    });

    const btn = $('.s1-btn');
    // Where the pointer clicks the subscribe button. The card is left
    // aligned and the avatar, name and button have fixed widths, so this
    // does not depend on the font (offsets are in frame units; the stage scale does not apply).
    const card = $('.s1-sub');
    const bx = card.offsetLeft + btn.offsetLeft + btn.offsetWidth * 0.4;
    const by = card.offsetTop + btn.offsetTop + btn.offsetHeight * 0.6;
    const tl = b.a(createTimeline({ defaults: { ease: 'outExpo' } }));
    tl.add('.s1-paused', { opacity: [0, 0.9, 0], scale: [0.8, 1, 1.15], duration: 700, ease: 'outQuad' }, S1_PAUSE * 1000)
      // Pointer slides in from the left edge, on a slight curve.
      .add(cursor, { left: [-80, tip(cx, cy).left], duration: 1100, ease: 'inOutCubic' }, 1850)
      .add(cursor, { top: [690, tip(cx, cy).top], duration: 1100, ease: 'outQuart' }, 1850)
      .add(arrow, { scale: [1, 0.78, 1], duration: 240, ease: 'inOutQuad' }, 3000)
      .add('.s1-ring', { r: [14, 70], opacity: [0.9, 0], strokeWidth: [5, 1], duration: 600, ease: 'outCubic' }, 3080)
      // Cut-out: the room goes dark, the creator stays lit and gets outlined.
      .call(() => root.classList.add('is-cut'), 3100)
      .add('.s1-dim', { opacity: [0, 0.8], duration: 650, ease: 'outQuad' }, 3100)
      .add(outline, { draw: ['0 0', '0 1'], duration: 850, ease: 'inOutQuad' }, 3100)
      // Push in towards the face once the outline is drawn.
      .add('.s1-shot', { scale: [1, 1.32], duration: 1500, ease: 'inOutCubic' }, 3500)
      // Subscribe card, then the pointer clicks it.
      .add('.s1-sub', { opacity: [0, 1], x: [-90, 0], duration: 600, ease: 'outBack(1.4)' }, 4000)
      .add(cursor, { ...tip(bx, by), duration: 900, ease: 'inOutCubic' }, 4150)
      .add(arrow, { scale: [1, 0.78, 1], duration: 240, ease: 'inOutQuad' }, 5100)
      .add(btn, { scale: [1, 0.9, 1], duration: 300, ease: 'inOutQuad' }, 5120)
      .call(() => root.classList.add('is-sub'), 5250)
      .add('.s1-bell', { opacity: [0, 1], scale: [0.4, 1], duration: 300, ease: 'outBack(2)' }, 5300)
      .add('.s1-bell', { rotate: [0, -22, 20, -14, 10, -5, 0], duration: 900, ease: 'linear' }, 5550)
      .add(cursor, { left: tip(bx + 60, by).left, top: tip(bx, by + 140).top, opacity: 0, duration: 700, ease: 'inQuad' }, 5900);

    return () => { b.stop(); video.pause(); root.classList.remove(...states); };
  }

  /* 2. Pixel art rocket leaving its planet. ----------------------------- */
  const P8 = {
    k: '#000000', n: '#1D2B53', p: '#7E2553', g: '#008751', o: '#AB5236', d: '#5F574F',
    l: '#C2C3C7', w: '#FFF1E8', r: '#FF004D', a: '#FFA300', y: '#FFEC27', b: '#29ADFF',
    v: '#83769C', i: '#FF77A8',
  };
  const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5].map((v) => (v + 0.5) / 16);
  const ROCKET = [
    '....r....',
    '...rwr...',
    '..rwwwr..',
    '..wwbww..',
    '..wbbbw..',
    '..wwbww..',
    '..wwwww..',
    '..wwlww..',
    '.rwwlwwr.',
    'rrwwlwwrr',
    'rr.ddd.rr',
  ];

  function pixel(root, lang) {
    const b = bag();
    const $ = html(root, `
      <canvas class="s2-canvas" width="256" height="144"></canvas>
      <div class="s2-count"></div>`);
    const cv = $('.s2-canvas');
    const cx = cv.getContext('2d');
    const count = $('.s2-count');

    // Static layers: dithered sky and lit planet, drawn once.
    const bg = document.createElement('canvas');
    bg.width = 256; bg.height = 144;
    const g = bg.getContext('2d');
    const sky = [P8.k, P8.n, P8.v];
    for (let y = 0; y < 144; y++) {
      for (let x = 0; x < 256; x++) {
        const t = (y / 144) * 1.6;
        const lvl = Math.min(2, Math.floor(t + BAYER[(y % 4) * 4 + (x % 4)] * 0.9));
        g.fillStyle = sky[lvl];
        g.fillRect(x, y, 1, 1);
      }
    }
    const pcx = 128, pcy = 236, pr = 128;
    const shades = [P8.k, P8.n, P8.p, P8.i, P8.w];
    for (let y = 100; y < 144; y++) {
      for (let x = 0; x < 256; x++) {
        const dx = x - pcx, dy = y - pcy;
        const d2 = dx * dx + dy * dy;
        if (d2 > pr * pr) continue;
        const nz = Math.sqrt(pr * pr - d2) / pr;
        const light = Math.max(0, (-dx / pr) * 0.45 + (-dy / pr) * 0.75 + nz * 0.35);
        const lvl = Math.min(4, Math.floor(light * 3.4 + BAYER[(y % 4) * 4 + (x % 4)]));
        g.fillStyle = shades[Math.max(1, lvl)];
        g.fillRect(x, y, 1, 1);
      }
    }
    [[70, 128, 9, 3], [168, 122, 6, 2], [205, 136, 11, 3], [108, 140, 7, 2]].forEach(([x, y, rx, ry]) => {
      g.fillStyle = P8.p;
      g.beginPath(); g.ellipse(x, y, rx, ry, 0, 0, Math.PI * 2); g.fill();
      g.fillStyle = P8.n;
      g.fillRect(x - rx + 2, y - ry + 1, rx * 2 - 4, 1);
    });
    // Moon.
    g.fillStyle = P8.l; g.beginPath(); g.arc(214, 30, 9, 0, Math.PI * 2); g.fill();
    g.fillStyle = P8.n; g.beginPath(); g.arc(218, 27, 9, 0, Math.PI * 2); g.fill();
    // Launch pad.
    g.fillStyle = P8.d; g.fillRect(118, 107, 21, 3);
    g.fillStyle = P8.l; g.fillRect(118, 107, 21, 1);
    g.fillStyle = P8.d; g.fillRect(140, 82, 2, 25); g.fillRect(136, 86, 4, 1); g.fillRect(136, 96, 4, 1);

    let seed = 7;
    const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    const stars = Array.from({ length: 70 }, () => ({
      x: Math.floor(rnd() * 256), y: Math.floor(rnd() * 100), p: rnd() * 6.28, c: rnd() < 0.2 ? P8.b : rnd() < 0.5 ? P8.l : P8.w,
    }));
    const smoke = [];
    const t0 = performance.now();
    const padY = 96; // top of the rocket sprite on the pad
    let last = 0;

    const words = lang === 'fr' ? ['3', '2', '1', 'DÉCOLLAGE'] : ['3', '2', '1', 'LIFT OFF'];
    words.forEach((w, i) => b.later(250 + i * 420, () => {
      count.textContent = w;
      count.classList.toggle('is-go', i === 3);
      b.a(animate(count, { scale: [1.6, 1], opacity: [1, i === 3 ? 1 : 0.85], duration: 380, ease: steps(4) }));
    }));
    b.later(3600, () => b.a(animate(count, { opacity: 0, duration: 300, ease: steps(3) })));

    b.raf((now) => {
      const t = (now - t0) / 1000;
      if (now - last < 1000 / 30) return; // 30 fps keeps the low-fi feel
      last = now;
      const lift = Math.max(0, t - 1.9);
      const ry = padY - 0.5 * 34 * lift * lift - lift * 6;
      const shake = t > 1.4 && t < 2.8 ? (Math.round(Math.sin(t * 90)) | 0) : 0;
      cx.drawImage(bg, shake, 0);
      stars.forEach((s) => {
        if (Math.sin(t * 3 + s.p) > -0.3) { cx.fillStyle = s.c; cx.fillRect(s.x, s.y, 1, 1); }
      });
      // Smoke puffs from the base.
      if (t > 1.3 && ry > -40) {
        for (let k = 0; k < 3; k++) {
          smoke.push({ x: 128 + (rnd() - 0.5) * 6, y: Math.min(108, ry + 15), vx: (rnd() - 0.5) * (lift > 0 ? 1.2 : 2.6), vy: lift > 0 ? 0.4 : -0.1, life: 0 });
        }
      }
      for (let k = smoke.length - 1; k >= 0; k--) {
        const s = smoke[k];
        s.x += s.vx; s.y += s.vy; s.vx *= 0.97; s.life += 1;
        if (s.y > 106) { s.y = 106; s.vx *= 1.04; }
        const size = Math.min(6, 1 + s.life / 6);
        cx.fillStyle = s.life < 4 ? P8.y : s.life < 10 ? P8.a : s.life < 40 ? P8.l : P8.d;
        cx.fillRect(Math.round(s.x - size / 2), Math.round(s.y - size / 2), Math.round(size), Math.round(size));
        if (s.life > 70) smoke.splice(k, 1);
      }
      // Rocket and flame.
      const rx = 124 + shake;
      const top = Math.round(ry);
      ROCKET.forEach((row, yy) => {
        for (let xx = 0; xx < row.length; xx++) {
          const c = row[xx];
          if (c !== '.') { cx.fillStyle = P8[c]; cx.fillRect(rx + xx, top + yy, 1, 1); }
        }
      });
      if (t > 1.3) {
        const f = Math.floor(t * 20) % 2;
        cx.fillStyle = P8.y; cx.fillRect(rx + 3, top + 11, 3, 2 + f);
        cx.fillStyle = P8.a; cx.fillRect(rx + 4, top + 13 + f, 1, 2 + (lift > 0 ? 3 : 0));
        if (lift > 0) { cx.fillStyle = P8.r; cx.fillRect(rx + 4, top + 16 + f, 1, 2); }
      }
    });
    return () => b.stop();
  }

  /* 3. Swiss type: a sales chart that climbs. --------------------------- */
  function swiss(root, lang) {
    const b = bag();
    const vals = [18, 24, 21, 33, 40, 51, 67, 100];
    const months = lang === 'fr'
      ? ['JAN', 'FÉV', 'MAR', 'AVR', 'MAI', 'JUN', 'JUL', 'AOÛ']
      : ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG'];
    const x0 = 96, bw = 92, gap = 26, base = 650, hMax = 400;
    const tops = vals.map((v, i) => [x0 + i * (bw + gap) + bw / 2, base - (v / 100) * hMax]);
    const $ = html(root, `
      <div class="s3-grid">${Array.from({ length: 12 }, () => '<i></i>').join('')}</div>
      <div class="s3-rule"></div>
      <div class="s3-label">${lang === 'fr' ? 'Ventes 2026<br>en milliers d’euros' : 'Sales 2026<br>in thousands of euros'}</div>
      <div class="s3-big"><span class="s3-num">0</span><span class="s3-pct">%</span></div>
      <div class="s3-bars">${vals.map((v, i) => `
        <div class="s3-bar${i === vals.length - 1 ? ' is-last' : ''}" style="left:${x0 + i * (bw + gap)}px;width:${bw}px;height:${(v / 100) * hMax}px"><b>${Math.round(v * 3.12)}</b></div>
        <div class="s3-month" style="left:${x0 + i * (bw + gap)}px;width:${bw}px">${months[i]}</div>`).join('')}</div>
      <svg class="s3-line" viewBox="0 0 ${W} ${H}"><polyline points="${tops.map((p) => p.join(',')).join(' ')}"/></svg>`);
    const line = svg.createDrawable($('.s3-line polyline'));
    utils.set(line, { draw: '0 0' });
    const num = { v: 0 };
    const out = $('.s3-num');
    const tl = b.a(createTimeline({ defaults: { ease: 'outExpo' } }));
    tl.add('.s3-grid i', { scaleY: [0, 1], duration: 700, delay: stagger(35) }, 0)
      .add('.s3-rule', { scaleX: [0, 1], duration: 800 }, 150)
      .add('.s3-label', { opacity: [0, 1], y: [24, 0], duration: 600 }, 350)
      .add('.s3-month', { opacity: [0, 1], duration: 300, delay: stagger(50) }, 500)
      .add('.s3-bar', { scaleY: [0, 1], duration: 1100, delay: stagger(110) }, 650)
      .add('.s3-bar b', { opacity: [0, 1], y: [10, 0], duration: 400, delay: stagger(110) }, 1300)
      .add(num, { v: [0, 212], duration: 2200, ease: 'outQuart', modifier: utils.round(0), onUpdate: () => { out.textContent = '+' + num.v; } }, 700)
      .add('.s3-big', { x: [-60, 0], opacity: [0, 1], duration: 900 }, 600)
      .add(line, { draw: ['0 0', '0 1'], duration: 1300, ease: 'inOutCubic' }, 2000)
      .add('.s3-bar.is-last', { scaleY: [1, 1.04, 1], duration: 500, ease: 'outBack(3)' }, 3200);
    return () => b.stop();
  }

  /* 4. A light bulb drawn in chalk that switches on. -------------------- */
  function chalk(root, lang) {
    const b = bag();
    const c = [560, 268];
    const rays = Array.from({ length: 9 }, (_, i) => {
      const a = (-200 + i * 27.5) * Math.PI / 180;
      const r1 = 175, r2 = 225 + (i % 2) * 18;
      return `<line x1="${c[0] + Math.cos(a) * r1}" y1="${c[1] + Math.sin(a) * r1}" x2="${c[0] + Math.cos(a) * r2}" y2="${c[1] + Math.sin(a) * r2}"/>`;
    }).join('');
    const hatch = Array.from({ length: 38 }, (_, i) => {
      const x = 300 + i * 11;
      return `<line x1="${x}" y1="430" x2="${x + 200}" y2="150"/>`;
    }).join('');
    const bulb = 'M502 384 C470 352 448 322 448 276 C448 214 498 164 560 164 C622 164 672 214 672 276 C672 322 650 352 618 384 L616 420 L504 420 Z';
    const $ = html(root, `
      <svg class="s4-svg" viewBox="0 0 ${W} ${H}">
        <defs>
          <filter id="s4-chalk" x="-5%" y="-5%" width="110%" height="110%">
            <feTurbulence type="fractalNoise" baseFrequency="1.1" numOctaves="2" seed="4" result="n"/>
            <feDisplacementMap in="SourceGraphic" in2="n" scale="5" xChannelSelector="R" yChannelSelector="G" result="d"/>
            <feColorMatrix in="n" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 -2.4 1.75" result="m"/>
            <feComposite in="d" in2="m" operator="in"/>
          </filter>
          <filter id="s4-dust"><feTurbulence type="fractalNoise" baseFrequency="0.012" numOctaves="3" seed="9"/>
            <feColorMatrix type="matrix" values="0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 0 0 0 0.09 0"/></filter>
          <clipPath id="s4-clip"><path d="${bulb}"/></clipPath>
          <radialGradient id="s4-glow"><stop offset="0" stop-color="#F4E58A" stop-opacity=".5"/><stop offset="1" stop-color="#F4E58A" stop-opacity="0"/></radialGradient>
        </defs>
        <rect width="${W}" height="${H}" filter="url(#s4-dust)"/>
        <circle class="s4-glow" cx="${c[0] + 70}" cy="${c[1] + 70}" r="330" fill="url(#s4-glow)"/>
        <g filter="url(#s4-chalk)" fill="none" stroke-linecap="round" stroke-linejoin="round" transform="translate(70 70)">
          <g class="s4-fill" clip-path="url(#s4-clip)" stroke="#F2DE6B" stroke-width="7">${hatch}</g>
          <g class="s4-ink" stroke="#EEF0E6" stroke-width="7">
            <path d="${bulb}"/>
            <path d="M520 384 V312 l13 -22 l13 22 l14 -22 l14 22 l13 -22 l13 22 V384"/>
            <path d="M500 442 H620"/><path d="M504 466 H616"/><path d="M510 490 H610"/>
            <path d="M532 508 Q560 532 588 508"/>
          </g>
          <g class="s4-rays" stroke="#F2DE6B" stroke-width="8">${rays}</g>
          <g class="s4-note" stroke="#EEF0E6" stroke-width="5">
            <path d="M900 250 C880 330 800 360 712 330"/><path d="M736 312 L710 330 L740 344"/>
          </g>
        </g>
      </svg>
      <div class="s4-word">${lang === 'fr' ? 'eurêka !' : 'eureka!'}</div>`);
    const ink = svg.createDrawable(root.querySelectorAll('.s4-ink path'));
    const fill = svg.createDrawable(root.querySelectorAll('.s4-fill line'));
    const rayD = svg.createDrawable(root.querySelectorAll('.s4-rays line'));
    const note = svg.createDrawable(root.querySelectorAll('.s4-note path'));
    utils.set([...ink, ...fill, ...rayD, ...note], { draw: '0 0' });
    utils.set('.s4-glow', { opacity: 0 });
    const tl = b.a(createTimeline({ defaults: { ease: 'inOutSine' } }));
    tl.add(ink[0], { draw: ['0 0', '0 1'], duration: 1300 }, 150)
      .add(ink.slice(1), { draw: ['0 0', '0 1'], duration: 380, delay: stagger(230) }, 1350)
      .add(fill, { draw: ['0 0', '0 1'], duration: 160, delay: stagger(26), ease: 'linear' }, 2600)
      .add('.s4-glow', { opacity: [0, 1], duration: 900, ease: 'outQuad' }, 2700)
      .add(rayD, { draw: ['0 0', '0 1'], duration: 260, delay: stagger(60), ease: 'outQuad' }, 2950)
      .add(note, { draw: ['0 0', '0 1'], duration: 420, delay: stagger(300) }, 3700)
      .add('.s4-word', { opacity: [0, 1], scale: [0.9, 1], duration: 500, ease: 'outBack(2)' }, 4300)
      .add('.s4-rays', { opacity: [1, 0.55, 1], duration: 700, loop: 3, ease: steps(2) }, 4600);
    return () => b.stop();
  }

  /* 5. A paper cut-out coffee cup, steaming, at 12 frames a second. ----- */
  function paper(root, lang) {
    const b = bag();
    const steam = [
      'M590 300 C560 260 620 230 590 190 C560 150 620 120 595 80',
      'M645 296 C615 250 675 220 645 175 C615 130 675 100 650 50',
      'M700 300 C670 262 730 232 700 192 C670 152 730 122 705 84',
    ];
    const $ = html(root, `
      <svg class="s5-svg" viewBox="0 0 ${W} ${H}">
        <defs>
          <filter id="s5-shadow" x="-20%" y="-20%" width="140%" height="160%">
            <feDropShadow dx="5" dy="8" stdDeviation="3" flood-color="#3A2614" flood-opacity=".32"/>
          </filter>
          <filter id="s5-grain"><feTurbulence type="fractalNoise" baseFrequency=".9" numOctaves="2" seed="2"/>
            <feColorMatrix type="matrix" values="0 0 0 0 .2 0 0 0 0 .13 0 0 0 0 .05 0 0 0 .22 0"/></filter>
          <clipPath id="s5-cup"><path d="M468 326 H812 C806 466 760 540 640 540 C520 540 474 466 468 326 Z"/></clipPath>
        </defs>
        <g class="j" filter="url(#s5-shadow)"><circle class="s5-sun" cx="930" cy="230" r="140" fill="#E4685A"/></g>
        <g class="j" filter="url(#s5-shadow)"><path class="s5-table" fill="#2E6B66" d="M0 560 L60 548 L140 562 L230 546 L330 560 L420 550 L520 563 L640 548 L760 561 L880 549 L990 562 L1100 547 L1200 560 L1280 552 V720 H0 Z"/></g>
        <g class="j" filter="url(#s5-shadow)"><g class="s5-beans">
          <g transform="translate(250 620) rotate(-20)"><ellipse rx="22" ry="15" fill="#5A3420"/><path d="M-16 0 Q0 -6 16 0" stroke="#2F1A0E" stroke-width="3" fill="none"/></g>
          <g transform="translate(320 655) rotate(35)"><ellipse rx="22" ry="15" fill="#6B3F26"/><path d="M-16 0 Q0 -6 16 0" stroke="#2F1A0E" stroke-width="3" fill="none"/></g>
          <g transform="translate(1010 640) rotate(10)"><ellipse rx="22" ry="15" fill="#5A3420"/><path d="M-16 0 Q0 -6 16 0" stroke="#2F1A0E" stroke-width="3" fill="none"/></g>
        </g></g>
        <g class="j" filter="url(#s5-shadow)"><ellipse class="s5-saucer" cx="640" cy="548" rx="240" ry="40" fill="#F6F1E7"/></g>
        <g class="j" filter="url(#s5-shadow)"><g class="s5-cup">
          <path d="M800 372 C905 352 912 482 788 478" fill="none" stroke="#F6F1E7" stroke-width="30" stroke-linecap="round"/>
          <path d="M468 326 H812 C806 466 760 540 640 540 C520 540 474 466 468 326 Z" fill="#F6F1E7"/>
          <g clip-path="url(#s5-cup)"><path d="M440 392 L840 380 L840 420 L440 432 Z" fill="#2E6B66"/></g>
          <ellipse cx="640" cy="328" rx="172" ry="24" fill="#EDE4D3"/>
          <ellipse cx="640" cy="332" rx="152" ry="17" fill="#5A3420"/>
        </g></g>
        <g class="s5-steam" fill="none" stroke="#FFFDF7" stroke-width="22" stroke-linecap="round">
          ${steam.map((d) => `<g class="j" filter="url(#s5-shadow)"><path d="${d}"/></g>`).join('')}
        </g>
        <rect width="${W}" height="${H}" filter="url(#s5-grain)" pointer-events="none"/>
      </svg>
      <div class="s5-tag">${lang === 'fr' ? 'noir, sans sucre' : 'black, no sugar'}</div>`);

    // Each cut-out sits in a .j group that gets a tiny random offset every
    // frame, like pieces nudged by hand between shots.
    const pieces = root.querySelectorAll('.j');
    const tag = $('.s5-tag');
    b.every(1000 / 12, () => {
      pieces.forEach((p) => {
        p.setAttribute('transform', `translate(${(Math.random() - 0.5) * 3} ${(Math.random() - 0.5) * 3})`);
      });
      tag.style.translate = `${(Math.random() - 0.5) * 3}px ${(Math.random() - 0.5) * 3}px`;
      tag.style.rotate = `${-4 + (Math.random() - 0.5) * 1.2}deg`;
    });

    const plumes = svg.createDrawable(root.querySelectorAll('.s5-steam path'));
    utils.set(plumes, { draw: '0 0' });
    const pop = { duration: 330, ease: steps(4) };
    const tl = b.a(createTimeline());
    tl.add('.s5-table', { y: [180, 0], ...pop }, 0)
      .add('.s5-sun', { scale: [0, 1], ...pop }, 250)
      .add('.s5-saucer', { y: [-260, 0], ...pop }, 500)
      .add('.s5-cup', { y: [-420, 0], ...pop }, 750)
      .add('.s5-beans > g', { opacity: [0, 1], duration: 1, delay: stagger(170) }, 1050)
      .add('.s5-tag', { opacity: [0, 1], duration: 1 }, 1650);
    b.later(1300, () => {
      plumes.forEach((p, i) => b.a(animate(p, {
        draw: ['0 0', '0 1', '1 1'], duration: 2000, delay: i * 420, loop: true, ease: steps(16),
      })));
    });
    return () => b.stop();
  }

  window.EnfaitScenes = [footage, pixel, swiss, chalk, paper];
})();
