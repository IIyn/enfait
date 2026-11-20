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

  /* 1. Live footage, a pointer clicks the singer's microphone. ---------- */
  function footage(root) {
    const b = bag();
    // Microphone head in the clip at about 2 s (frame coordinates).
    const mx = 652;
    const my = 378;
    const arcs = [70, 110, 150]
      .map((r) => `<path d="M ${mx - r * 0.5} ${my - r * 0.87} A ${r} ${r} 0 0 0 ${mx - r * 0.5} ${my + r * 0.87}"/>` +
        `<path d="M ${mx + r * 0.5} ${my - r * 0.87} A ${r} ${r} 0 0 1 ${mx + r * 0.5} ${my + r * 0.87}"/>`)
      .join('');
    const bars = Array.from({ length: 28 }, () => '<i></i>').join('');
    const $ = html(root, `
      <video class="s1-video" src="media/singer.mp4" poster="media/singer.jpg" muted playsinline loop preload="auto"></video>
      <svg class="s1-fx" viewBox="0 0 ${W} ${H}">
        <g class="s1-rings">${[0, 1, 2].map(() => `<circle cx="${mx}" cy="${my}" r="18"/>`).join('')}</g>
        <g class="s1-arcs">${arcs}</g>
      </svg>
      <div class="s1-meter">${bars}</div>
      <div class="s1-rec"><span></span>ON</div>
      <div class="s1-cursor"><svg viewBox="0 0 24 24" width="54" height="54">
        <path d="M4 2.5v17.2l4.3-4.1 3 6.7 3.1-1.4-3-6.6h6z" fill="#fff" stroke="#0E1110" stroke-width="1.5" stroke-linejoin="round"/>
      </svg></div>`);
    const video = $('.s1-video');
    video.currentTime = 0;
    video.play().catch(() => {});

    const cursor = $('.s1-cursor');
    // Tip of the arrow is at (4, 2.5) in a 24 unit box drawn at 54 px.
    const tipX = mx - 9;
    const tipY = my - 5.6;
    const drawArcs = svg.createDrawable(root.querySelectorAll('.s1-arcs path'));
    utils.set(drawArcs, { draw: '0 0' });

    const tl = b.a(createTimeline({ defaults: { ease: 'outExpo' } }));
    tl.add(cursor, { left: [W + 60, tipX], duration: 1300, ease: 'inOutCubic' }, 450)
      .add(cursor, { top: [H + 80, tipY], duration: 1300, ease: 'outQuart' }, 450)
      .add(cursor.firstElementChild, { scale: [1, 0.78, 1], duration: 260, ease: 'inOutQuad' }, 1900)
      .call(() => root.classList.add('is-on'), 2000)
      .add('.s1-rings circle', { r: [18, 190], opacity: [0.95, 0], strokeWidth: [6, 1], duration: 1100, delay: stagger(140), ease: 'outCubic' }, 2000)
      .add(drawArcs, { draw: ['0 0', '0 1'], duration: 520, delay: stagger(60) }, 2050)
      .add(video, { scale: [1, 1.045], duration: 700, ease: 'outBack(2)' }, 2000)
      .add('.s1-rec', { opacity: [0, 1], x: [-16, 0], duration: 400 }, 2150)
      .add('.s1-meter', { opacity: [0, 1], duration: 300 }, 2100)
      .add(cursor, { left: tipX + 120, top: tipY + 90, duration: 1400, ease: 'inOutSine' }, 3000);

    const meter = root.querySelectorAll('.s1-meter i');
    b.later(2100, () => b.every(95, () => {
      meter.forEach((m, i) => {
        const base = 0.25 + 0.55 * Math.abs(Math.sin(i * 0.5 + performance.now() / 300));
        m.style.transform = `scaleY(${Math.min(1, base * (0.55 + Math.random() * 0.7)).toFixed(2)})`;
      });
    }));
    return () => { b.stop(); video.pause(); };
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
