# Guide de motion design d'enfait

Ces règles valent pour toutes les vidéos. Les règles de style de
l'utilisateur, quand il y en a, passent avant.

## Ce qu'on fabrique

Des scènes animées qui accompagnent une voix off : vidéos YouTube, tutoriels,
explications, présentations de produit. L'image **montre** ce que la voix
**dit**, au moment où elle le dit. Le spectateur ne lit pas un diaporama : il
écoute, et l'image rend l'idée évidente.

## Une scène, une idée

- Un seul point focal à la fois. Si deux choses bougent, l'une mène, l'autre
  accompagne.
- On n'écrit jamais la phrase entière à l'écran : les sous-titres sont une
  couche à part. À l'écran, seulement les mots clés, les chiffres, les noms.
  Six mots au maximum visibles en même temps, sauf demande explicite.
- On préfère le concret à l'abstrait : un schéma qui se construit, un
  graphique qui pousse, une interface dessinée, un compteur, une carte, un
  objet en SVG. Une métaphore visuelle simple vaut mieux qu'un titre.

## Le temps

- **Calage sur la voix.** Un élément lié à un mot apparaît sur ce mot, en
  arrivant 2 à 4 images avant lui (environ 0,1 s) pour être en place quand on
  l'entend. Les instants des mots sont fournis : sers-t'en.
- Entrées : 0,35 à 0,7 s. Sorties : 0,25 à 0,4 s. Un texte important reste
  lisible au moins 0,8 s.
- Rien n'est tout à fait immobile : une dérive lente (échelle 1 → 1,03, ou un
  léger déplacement) garde la scène vivante entre deux événements.
- La scène est posée à sa fin : pas d'animation coupée en plein vol au
  changement de bloc, sauf si la direction prévoit une continuité. Ne vide
  pas l'écran à la fin : le montage s'occupe du passage au bloc suivant.
- Le rythme varie : une scène dense alterne avec une scène qui respire. À
  l'intérieur d'une scène aussi, des moments rapides et des moments posés.
- Pas de temps mort de plus d'une seconde sans événement visuel, et jamais
  d'écran vide au début : quelque chose est visible dès 0,2 s.
- À la fin de la scène, tout texte visible est complet et lisible (pas de
  brouillage ou de frappe en cours).

## Le mouvement

- Courbes : `outCubic` / `outExpo` pour les entrées, `inQuad` pour les
  sorties, `inOutCubic` / `inOutExpo` pour les déplacements. Linéaire
  seulement pour les mouvements continus (rotation lente, défilement). Un
  dépassement (`outBack(1.4)`) de temps en temps, pas partout.
- Décalages (`stagger`) entre 20 et 80 ms pour les lettres, 60 à 150 ms pour
  les mots ou les éléments d'une liste.
- Révélations par masque plutôt que par fondu : lettres ou lignes qui montent
  derrière un masque (`splitText` avec `wrap: "clip"`), `clipPath`, tracés
  qui se dessinent (`svg.createDrawable`), formes qui se transforment
  (`svg.morphTo`).
- On anime `transform` et `opacity` de préférence.
- La caméra peut bouger : un conteneur global qui zoome ou glisse vers
  l'élément suivant enchaîne mieux qu'un écran qui se vide.

## La composition

- On utilise tout le cadre. Mises en page asymétriques, grandes masses,
  espace vide assumé. Aligne sur une grille (marges de 5 % minimum).
- Hiérarchie nette : trois tailles de texte au plus. À 1080 p : 120 à 220 px
  pour un mot clé, 56 à 90 px pour un titre, 36 px minimum pour du texte
  secondaire.
- Deux familles de polices au plus : une pour l'affichage, une pour le texte.
  Approche serrée (`letter-spacing` négatif) sur les grands titres, chiffres
  tabulaires pour les compteurs.
- Couleurs : celles de la direction artistique. L'accent sert à l'élément le
  plus important de la scène, pas à la décoration.
- Les icônes et illustrations sont dessinées en SVG inline, traits de même
  épaisseur, bouts arrondis, cohérentes d'une scène à l'autre.

## Ce qu'on ne fait pas

Ces tics donnent un rendu générique, « fait par une IA » :

- dégradés violet-bleu, halos lumineux, néons, verre dépoli, ombres floues
  colorées ;
- particules ou étincelles qui flottent sans raison, bulles décoratives ;
- emojis, cliparts, texte d'exemple, faux logos ;
- tout arrondi en pilule, cartes empilées partout ;
- texte qui rebondit sans raison, rotations 3D gratuites ;
- une phrase entière centrée qui apparaît en fondu.

## Avant de rendre

- Vérifie mentalement les instants clés : à `t = 0`, au premier mot clé, au
  milieu, à la fin. Chaque image doit être une belle image fixe.
- Le texte lisible reste dans la zone sûre.
- La durée déclarée est exactement celle demandée.
