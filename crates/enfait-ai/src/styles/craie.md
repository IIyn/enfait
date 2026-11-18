# Tableau à la craie

## Essence
Un professeur passionné explique au tableau. On voit l'idée se construire
trait par trait, avec des flèches, des encadrés et des ratures. C'est vivant,
imparfait et clair.

## Matière et rendu
- Fond ardoise `#2B3A33`, avec des traces d'effacement fixes et très
  discrètes.
- Traits de craie : contours SVG au tracé légèrement irrégulier (petites
  variations calculées une fois sur les points du chemin), un motif de
  grain fixe posé en masque, et une légère transparence irrégulière. Jamais de remplissage plein : on remplit avec
  des hachures rapides.
- Épaisseur constante d'environ 5 px, bouts arrondis.

## Couleur
Craie blanche `#ECEBE4` pour l'essentiel. Deux craies de couleur au plus :
jaune `#F2D46B` pour souligner, rose `#E8908E` pour corriger ou opposer.

## Typographie
« Caveat » pour tout ce qui est écrit à la main, graisse 500 à 700. Les mots
s'écrivent lettre par lettre (`splitText` puis apparition décalée de 40 à
70 ms). Les titres sont soulignés à la main.

## Mouvement
- Tout se dessine dans l'ordre où une main le ferait (`svg.createDrawable`),
  à vitesse d'écriture.
- Les flèches partent d'un élément existant vers le nouveau.
- La caméra recadre le tableau pour suivre la main, avec des panoramiques
  courts.

## Gestes propres au style
- Entourer un mot clé d'un trait qui ne se referme pas tout à fait.
- Rayer une idée fausse, puis écrire la bonne au-dessus.
- Un coup d'éponge efface le tableau (transition).

## Pièges
- Des traits parfaitement réguliers : on dirait du vectoriel.
- Tout écrire en même temps : la craie est séquentielle.
- Trop de texte : un tableau, ce sont des mots clés et des schémas.

## Liberté
Schémas, ordre de construction, place des éléments : libres.
