# Gravure d'archives

## Essence
Une planche de dictionnaire du XIXᵉ siècle qui s'anime. Tout est trait :
hachures, tailles-douces, filets d'encadrement. Le savoir a l'air ancien,
précis et un peu solennel. Pas un « effet vieux papier » : une vraie
logique d'imprimerie.

## Matière et rendu
- Fond papier chiffon, autour de `#EFE8D8`, légèrement irrégulier : un
  grain très doux calculé une fois dans un canvas, fixe.
- Formes construites en hachures parallèles ou croisées (motifs SVG
  `pattern` de lignes), jamais en aplats. Le volume vient de la densité des
  traits, pas d'un dégradé.
- Encre `#1E1B17` à pleine opacité. Les gris sont des hachures, pas de la
  transparence.
- Filets doubles pour encadrer, lettrines, numéros de planche (« Pl. III »).

## Couleur
Une seule couleur d'accent, posée comme un tampon ou une cire : vermillon
`#B23A2A`. Elle désigne une seule chose par scène.

## Typographie
Titres en « Fraunces » (opsz élevé, graisse 500 à 700), légendes en italique
« Instrument Serif ». Petites capitales espacées pour les numéros et dates.
Jamais de sans-serif.

## Mouvement
- Les traits se dessinent (`svg.createDrawable`), au rythme d'un graveur :
  rapides, réguliers, de gauche à droite.
- Les hachures arrivent par couches successives, comme des passages du
  burin.
- Pas de rebond, pas d'élasticité. Courbes `outCubic`, `inOutQuad`.
- La caméra glisse lentement sur la planche, comme un regard qui lit.

## Gestes propres au style
- Un renvoi numéroté (« fig. 2 ») relie un détail à sa légende par un filet
  qui se trace.
- Un médaillon ovale se découpe dans la planche pour isoler un portrait.
- Une page tourne en révélant la planche suivante (transition).

## Pièges
- Hachures trop régulières : on dirait une grille ou un code-barres. Varie
  l'espacement selon la forme.
- Texture papier animée : ça scintille. Elle reste fixe.
- Sépia partout : c'est un filtre, pas une gravure.

## Liberté
Composition, sujet des planches, nombre de figures, place du texte : libres.
