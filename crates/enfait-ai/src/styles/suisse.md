# Typographie suisse

## Essence
Le style typographique international : grille stricte, asymétrie, texte
comme seul matériau. L'information est belle parce qu'elle est ordonnée.
Rigueur, contraste, aucune décoration.

## Matière et rendu
- Grille de 12 colonnes visible dans les décisions, jamais dessinée.
- Aplats purs, filets fins (2 px) et gros chiffres. Pas de texture, pas
  d'ombre, pas d'illustration figurative : des formes géométriques
  primaires au besoin.

## Couleur
Fond blanc cassé `#F4F2EE` ou noir `#111111`. Une couleur forte : rouge
`#E63027`. Le reste en noir et gris.

## Typographie
« Inter » uniquement, graisses contrastées (400 et 800). Alignement à gauche,
fer à droite. Très grandes tailles pour un mot ou un chiffre (300 px et plus),
interlignage serré, approche négative. Les textes secondaires sont petits et
nombreux, comme des légendes.

## Mouvement
- Les éléments glissent le long de la grille : déplacements horizontaux ou
  verticaux uniquement, jamais en diagonale.
- Révélations par masque rectangulaire (`clipPath: inset(...)`).
- Courbes nettes : `inOutExpo`, `outQuart`. Arrêts francs.
- Les changements de mise en page se font en réarrangeant les mêmes blocs,
  pas en les remplaçant.

## Gestes propres au style
- Un chiffre géant défile (compteur) pendant que sa légende s'écrit à côté.
- Une colonne entière se décale d'un module et pousse les autres.
- Le cadre se divise en deux par un filet qui traverse l'écran (transition).

## Pièges
- Centrer le texte : c'est tout l'inverse du style.
- Ajouter des icônes ou des illustrations : le texte suffit.
- Mouvements doux et flottants : le style est mécanique et précis.

## Liberté
Hiérarchie, mots mis en avant, rythme de la grille : libres.
