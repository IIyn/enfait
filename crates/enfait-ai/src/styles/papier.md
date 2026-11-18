# Papier découpé

## Essence
Une animation en papier découpé filmée image par image : des formes simples,
des bords nets, des ombres portées courtes. Ça a l'air fait à la main sur une
table, avec patience et humour.

## Matière et rendu
- Chaque forme est un morceau de papier : aplat de couleur, bord légèrement
  irrégulier (chemin SVG avec de petites aspérités, jamais un cercle parfait).
- Ombre portée courte et dure sous chaque élément (`drop-shadow` 3 à 6 px,
  noir à 25 %), même direction partout.
- Fond : un papier de couleur, texture fibreuse discrète et fixe.

## Couleur
Quatre à cinq papiers maximum, saturés mais pas criards (par exemple ocre
`#D9A441`, bleu `#2F5D8C`, rouge brique `#C4553A`, crème `#F3E9D2`, encre
`#22201C`). Pas de dégradés : le papier est uni.

## Typographie
Lettres découpées : « Archivo » très gras (graisse 800 à 900, chasse
étroite), ou des mots posés comme des étiquettes. Chaque lettre peut être un
morceau de papier à part.

## Mouvement
- **Animé sur les deux** : l'image change 12 fois par seconde. Dans `seek`,
  arrondis le temps : `tl.seek(Math.floor(t * 12) / 12 * 1000)`.
- Les formes glissent, pivotent et se posent ; elles ne se déforment pas et
  ne changent pas d'échelle de façon continue.
- Un léger tremblé (rotation ±0,6°, décalage ±1 px) rend la main visible.
  Il change toutes les deux images et se calcule avec
  `enfaitNoise(Math.floor(t * 12) * 7 + indexDeLaForme)` : jamais de
  `Math.random()` dans `seek`.

## Gestes propres au style
- Une forme entre du bord du cadre comme poussée par un doigt.
- Un pli : une forme se replie (`scaleX` vers 0, avec une face plus sombre).
- Une déchirure révèle la scène suivante (transition).

## Pièges
- Animation fluide à 30 i/s : on perd l'artisanat.
- Ombres de directions différentes : la table n'est plus crédible.
- Trop de détails : le papier découpé vit de formes simples.

## Liberté
Personnages, décors, métaphores, cadrage : libres.
