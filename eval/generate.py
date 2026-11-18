#!/usr/bin/env python3
"""Génère eval/cases.json : les blocs de référence du jeu d'évaluation.

Les instants des mots sont synthétiques mais réalistes (débit d'environ six
syllabes par seconde, pauses aux virgules et aux points), et déterministes :
relancer le script donne le même fichier. Modifier les cas ici, puis
`python3 eval/generate.py`.
"""

import json
import re
from pathlib import Path

# id, ce que le cas met à l'épreuve, style (None = libre), format, langue,
# contexte (sujet de la vidéo), texte dit par la voix.
CASES = [
    ("chiffre-choc", "un grand nombre isolé", None, "16:9", "fr",
     "Vidéo de vulgarisation sur l'eau douce.",
     "Chaque année, nous prélevons 4000 milliards de mètres cubes d'eau douce."),
    ("pourcentage", "une proportion à rendre visible", "suisse", "16:9", "fr",
     "Vidéo sur la consommation d'énergie des centres de données.",
     "Les centres de données consomment déjà près de 2 % de l'électricité mondiale."),
    ("liste-trois", "une énumération de trois éléments", None, "16:9", "fr",
     "Tutoriel sur le pain au levain.",
     "Il ne faut que trois ingrédients : de la farine, de l'eau, et du sel."),
    ("etapes", "un processus en étapes", "craie", "16:9", "fr",
     "Cours d'introduction à la photosynthèse.",
     "La plante capte la lumière, absorbe le gaz carbonique, puis rejette de l'oxygène."),
    ("citation", "une citation attribuée", "gravure", "16:9", "fr",
     "Documentaire sur l'histoire des sciences.",
     "Newton écrivait : si j'ai vu plus loin, c'est en montant sur les épaules de géants."),
    ("date-histoire", "une date et un événement", "gravure", "16:9", "fr",
     "Documentaire sur les grandes expéditions.",
     "Le 14 décembre 1911, Roald Amundsen atteint le pôle Sud, cinq semaines avant Scott."),
    ("abstrait", "une idée abstraite sans objet concret", None, "16:9", "fr",
     "Essai vidéo sur la prise de décision.",
     "Choisir, c'est toujours renoncer à quelque chose."),
    ("emotion", "une émotion, de la retenue", "papier", "16:9", "fr",
     "Récit personnel sur un déménagement.",
     "Quand j'ai fermé la porte pour la dernière fois, la maison m'a semblé minuscule."),
    ("question", "une question rhétorique", None, "9:16", "fr",
     "Vidéo courte pour les réseaux sur le sommeil.",
     "Pourquoi est-ce qu'on se réveille toujours juste avant le réveil ?"),
    ("avant-apres", "un contraste avant / après", "suisse", "16:9", "fr",
     "Présentation d'une refonte de site.",
     "Avant, il fallait onze clics pour payer. Aujourd'hui, il en faut deux."),
    ("geographie", "un lieu, une carte", "papier", "16:9", "fr",
     "Vidéo de voyage sur l'Islande.",
     "L'Islande se trouve exactement sur la frontière entre deux plaques tectoniques."),
    ("personne", "présenter une personne", None, "16:9", "fr",
     "Portrait d'une chercheuse.",
     "Rosalind Franklin a photographié l'ADN avant que quiconque ne comprenne sa forme."),
    ("explication", "un mécanisme scientifique", "craie", "16:9", "fr",
     "Cours de physique sur les arcs-en-ciel.",
     "Chaque goutte de pluie agit comme un petit prisme : elle sépare la lumière blanche en couleurs."),
    ("metaphore", "une métaphore filée", None, "16:9", "fr",
     "Vidéo sur la mémoire humaine.",
     "La mémoire n'est pas un disque dur. C'est plutôt un conteur qui réécrit l'histoire à chaque fois."),
    ("tres-court", "un bloc très court et percutant", None, "16:9", "fr",
     "Introduction d'une vidéo sur la procrastination.",
     "Demain. Toujours demain."),
    ("long", "un bloc long avec beaucoup d'information", "suisse", "16:9", "fr",
     "Explication du fonctionnement d'une élection.",
     "Le scrutin se déroule en deux tours. Si aucun candidat n'obtient la majorité absolue au "
     "premier tour, les deux premiers s'affrontent quinze jours plus tard, et celui qui "
     "recueille le plus de voix l'emporte."),
    ("classement", "un classement, un podium", None, "16:9", "fr",
     "Vidéo sur les langues les plus parlées.",
     "En tête, l'anglais. Juste derrière, le mandarin. Et en troisième position, l'hindi."),
    ("frise", "une chronologie", "gravure", "16:9", "fr",
     "Histoire de l'écriture.",
     "L'écriture apparaît vers 3300 avant notre ère, l'alphabet vers 1 200, et l'imprimerie en 1450."),
    ("cause-effet", "une chaîne de causes", "papier", "16:9", "fr",
     "Vidéo sur les écosystèmes.",
     "Moins d'abeilles, c'est moins de fleurs pollinisées, et donc moins de fruits."),
    ("avertissement", "une mise en garde", None, "1:1", "fr",
     "Conseils de cybersécurité.",
     "Ne réutilisez jamais le même mot de passe sur deux sites différents."),
    ("appel-action", "une fin de vidéo, un appel", None, "9:16", "fr",
     "Fin d'une vidéo de cuisine.",
     "Essayez la recette ce week-end, et dites-moi ce que vous en pensez."),
    ("definition", "définir un mot", "suisse", "16:9", "fr",
     "Lexique de l'économie.",
     "L'inflation, c'est la hausse générale et durable des prix."),
    ("compteur", "un nombre qui monte", None, "16:9", "fr",
     "Vidéo sur la croissance d'une ville.",
     "En cinquante ans, la ville est passée de 20000 à plus de 2 millions d'habitants."),
    ("argent", "une somme d'argent", None, "16:9", "fr",
     "Vidéo sur l'économie du jeu vidéo.",
     "Ce jeu a coûté 300 millions de dollars, plus que la plupart des films hollywoodiens."),
    ("echelle", "une différence d'échelle", "craie", "16:9", "fr",
     "Vidéo sur l'infiniment petit.",
     "Si un atome avait la taille d'un stade, son noyau serait une bille posée au centre."),
    ("vitesse", "une vitesse, du mouvement", None, "16:9", "fr",
     "Vidéo sur les animaux les plus rapides.",
     "Le faucon pèlerin plonge à plus de 380 kilomètres à l'heure."),
    ("technique", "un concept logiciel", None, "16:9", "fr",
     "Tutoriel de programmation.",
     "Un cache garde une copie des données récentes, pour éviter de les recalculer."),
    ("en-sport", "anglais, un exploit sportif", None, "16:9", "en",
     "Sports history short.",
     "In 1954, Roger Bannister ran a mile in under four minutes, something doctors called impossible."),
    ("en-nature", "anglais, la nature", "papier", "9:16", "en",
     "Nature short about forests.",
     "Beneath every forest lies a hidden network of fungi, quietly sharing food between the trees."),
    ("en-produit", "anglais, présentation de produit", "suisse", "16:9", "en",
     "Launch video for a note-taking app.",
     "Write it once. Find it anywhere. Your notes, finally organised."),
]

VOWELS = "aeiouyàâäéèêëîïôöûùüœæ"
SECONDS_PER_SYLLABLE = 0.165
GAP = 0.05
COMMA_PAUSE = 0.28
SENTENCE_PAUSE = 0.55
LEAD_IN = 0.35
TAIL = 0.6


def syllables(word: str, lang: str) -> float:
    w = word.lower()
    digits = sum(c.isdigit() for c in w)
    letters = re.sub(r"[^a-zàâäéèêëîïôöûùüœæç]", "", w)
    if lang == "fr" and len(letters) > 3 and letters.endswith(("e", "es", "ent")):
        letters = re.sub(r"(e|es|ent)$", "", letters)
    if lang == "en" and len(letters) > 3 and letters.endswith("e"):
        letters = letters[:-1]
    groups = len(re.findall(f"[{VOWELS}]+", letters))
    return max(1.0, groups + 1.4 * digits) if letters or digits else 0.5


def time_words(text: str, lang: str):
    t = LEAD_IN
    words = []
    for raw in text.split():
        duration = max(0.12, syllables(raw, lang) * SECONDS_PER_SYLLABLE)
        if raw in {"%", ":", "?"}:
            # Signe détaché du mot précédent (typographie française).
            if words:
                words[-1]["text"] += " " + raw
            t += COMMA_PAUSE if raw == ":" else 0.0
            continue
        words.append({"t": round(t, 2), "text": raw})
        t += duration + GAP
        if raw.endswith((",", ":", ";")):
            t += COMMA_PAUSE
        elif raw.endswith((".", "!", "?", "…")):
            t += SENTENCE_PAUSE
    # Le dernier mot finit sa phrase : la pause finale est remplacée par la traîne.
    end = t - (SENTENCE_PAUSE if text.rstrip().endswith((".", "!", "?")) else 0) + TAIL
    return words, round(end, 2)


def main():
    out = []
    for cid, theme, style, aspect, lang, context, text in CASES:
        words, duration = time_words(text, lang)
        out.append({
            "id": cid,
            "theme": theme,
            "style": style,
            "aspect": aspect,
            "language": lang,
            "context": context,
            "text": " ".join(w["text"] for w in words),
            "duration": duration,
            "words": words,
        })
    path = Path(__file__).with_name("cases.json")
    path.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{len(out)} cas, {sum(c['duration'] for c in out):.0f} s au total → {path}")


if __name__ == "__main__":
    main()
