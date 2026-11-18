# Contrat technique d'une composition enfait

Une composition est **un seul document HTML autonome** qui anime une scène de
durée fixe. Elle est rendue image par image par un Chromium sans écran, puis
encodée en vidéo. Rien n'est joué en temps réel.

## Ce que la page doit déclarer

```js
window.enfait = {
  duration: 4.2,                       // secondes, égale à la durée demandée
  seek(t) { tl.seek(t * 1000); },      // t en secondes ; anime.js compte en ms
};
```

- `seek(t)` doit être **pur** : le même `t` donne toujours la même image, quel
  que soit l'ordre des appels (le moteur peut revenir en arrière).
- Le plus simple : une seule timeline anime.js **sans lecture automatique**
  (`createTimeline({ autoplay: false })`), construite au chargement, et
  `seek(t)` qui appelle `tl.seek(t * 1000)`.
- Pas de `setTimeout`, `setInterval`, `requestAnimationFrame`, `Date.now()`
  ni d'`animate(...)` qui joue tout seul : seul `t` fait avancer la scène.
- `Math.random()` est déterministe au chargement, mais **pas dans `seek`**
  (le résultat dépendrait de l'ordre des appels). Pour du bruit qui dépend
  du temps, utilise `enfaitNoise(n)`, qui renvoie toujours la même valeur
  dans [-1, 1] pour un même entier `n` (par exemple
  `enfaitNoise(Math.floor(t * 12))`).

## Environnement

- Taille de la page : celle indiquée dans la demande (par exemple 1920×1080).
  Dimensionne en `px` sur cette base. Pas de défilement
  (`overflow: hidden` sur `html, body`).
- Déjà chargés pour toi, **ne les inclus pas** :
  - anime.js 4 (global `anime`) ;
  - les polices listées dans la demande (`font-family: "Inter"` etc.).
- **Aucun accès réseau** : pas de CDN, pas de Google Fonts, pas d'image
  distante. Dessine en HTML/CSS/SVG inline.
- Pas d'audio ni de `<video>` (les vidéos de l'utilisateur servent aux plans
  de b-roll, montés à part).
- **Textures** (grain, papier, bruit) : calcule-les une seule fois au
  chargement (dans un `<canvas>` converti en image de fond, ou un petit
  motif répété), puis laisse-les fixes. Pas de filtre SVG `feTurbulence` ou
  `feDisplacementMap` sur une grande surface : il est recalculé à chaque
  image et ralentit le rendu.

## anime.js 4 : l'essentiel

C'est la **version 4**, dont l'API diffère de la v3 (pas de `anime({...})`,
pas de `anime.timeline`, pas de `targets:` dans les paramètres).

```js
const { createTimeline, stagger, splitText, svg, utils } = anime;

const tl = createTimeline({ autoplay: false, defaults: { ease: "outExpo", duration: 600 } });

// tl.add(cibles, propriétés, position)
tl.add(".titre", { y: ["40px", "0px"], opacity: [0, 1] }, 0)      // à 0 ms
  .add(".ligne", { scaleX: [0, 1], ease: "inOutExpo" }, 350)       // à 350 ms
  .add(".tag", { opacity: [0, 1] }, "-=200")                       // 200 ms avant la fin du précédent
  .add(".fond", { scale: [1, 1.03], ease: "linear", duration: 4200 }, 0);
tl.set(".cache", { opacity: 0 }, 2000);                            // changement instantané
```

- **Temps en millisecondes.** Position : nombre (ms absolues depuis le début
  de la timeline), `"+=200"` / `"-=200"` (relatif à la fin de l'ajout
  précédent), `"<"` (fin du précédent), `"<<"` (début du précédent). Préfère
  les positions absolues calées sur les instants des mots.
- **Propriétés** : transformations raccourcies `x`, `y`, `scale`, `scaleX`,
  `rotate`, `skewX` ; CSS en camelCase (`opacity`, `clipPath`,
  `backgroundColor`, `letterSpacing`) ; variables CSS (`"--p"`) ; attributs
  SVG (`r`, `cx`, `strokeWidth`). Valeur de départ et d'arrivée :
  `[from, to]`. Unités comprises : `"110%"`, `"2rem"`, `"-4deg"`.
- **Courbes** (`ease`) : `"linear"`, `"inQuad"`, `"outQuad"`, `"inOutQuad"`,
  `"outCubic"`, `"outQuart"`, `"outExpo"`, `"inOutExpo"`, `"inBack"`,
  `"outBack(1.4)"`, `"outElastic(1, .6)"`, `"out(3)"`, `"inOut(4)"`,
  `"steps(6)"`. Pas de noms GSAP (`power3.out` n'existe pas ici).
- **Décalages** : `delay: stagger(40)`, `stagger(60, { from: "center" })`,
  `stagger(50, { start: 300 })`, `stagger([0, 400])` (répartis sur 400 ms).
- **Images clés** : `y: [{ to: -30, duration: 250 }, { to: 0, ease: "outBounce" }]`.
- **Texte découpé** :
  ```js
  const { lines, words, chars } = splitText(".titre", {
    lines: true, words: true, chars: { wrap: "clip" },  // "clip" : masque pour révéler
  });
  tl.add(chars, { y: ["110%", "0%"], delay: stagger(30) }, 200);
  ```
- **Tracés SVG** : `tl.add(svg.createDrawable("#chemin"), { draw: ["0 0", "0 1"], duration: 1200 }, 0)`.
- **Morphing** : `tl.add("#forme", { d: svg.morphTo("#cible") }, 800)` (ou
  `points` pour un polygone).
- **Suivi de trajectoire** :
  `tl.add(".point", { ease: "linear", duration: 2000, ...svg.createMotionPath("#trajet") }, 0)`.
- **Compteurs** : anime un objet et écris le texte dans `onUpdate`, appelé
  aussi pendant `seek` :
  ```js
  const n = { v: 0 };
  tl.add(n, { v: 1250, duration: 1200, modifier: utils.round(0),
    onUpdate: () => { el.textContent = String(n.v).replace(/\B(?=(\d{3})+(?!\d))/g, " "); } }, 300);
  ```
  N'utilise pas `toLocaleString` (séparateurs invisibles dans certaines
  polices).
- Les éléments animés doivent exister quand la timeline est construite.
  Évite `onComplete`, `onBegin` et toute logique qui dépend de l'ordre de
  lecture.

## Images fournies par le moteur (transitions, scènes avec la prise)

Une composition de transition reçoit les deux plans qu'elle relie, et une
scène « mixte » ou « effet » reçoit la prise filmée (même image dans `from`
et `to`), sous forme d'images mises à jour à chaque frame par le moteur :

```html
<img data-enfait-scene="from" style="position:absolute; inset:0; width:100%; height:100%">
<img data-enfait-scene="to"   style="position:absolute; inset:0; width:100%; height:100%">
```

Ne fixe pas leur `src`. Anime-les comme des calques (masques, `clipPath`,
transformations, opacité). Pour une transition, à `t = 0` on ne doit voir
que `from`, à `t = duration` que `to`. Pour une scène avec la prise,
n'utilise que `from` (masque `to`) et ne déforme pas la personne filmée,
sauf si la demande dit que `to` porte la personne détourée : c'est alors
un calque transparent aligné sur `from`, à poser au-dessus de tes éléments.

## Rendu attendu

- Le fond de page est opaque (définis `background` sur `html, body`).
- Le texte important reste dans la **zone sûre** (5 % de marge sur chaque
  bord) quand il est lisible.
- Le document complet tient dans un seul bloc ```` ```html ````.
