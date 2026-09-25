---
name: godot-doc-search
description: Rechercher des réponses dans la documentation Godot 4.7 stable (manuel + référence des classes, instantané local en anglais) via l'outil md-doc-search, hors-ligne. À utiliser dès que l'utilisateur pose une question sur une classe, méthode, propriété, signal, nœud, shader ou du GDScript, demande « comment faire X dans Godot », ou AVANT d'écrire tout appel API Godot — même si la question est en français : la recherche se fait toujours avec des mots-clés ANGLAIS.
---

# Godot Doc Search (corpus local, moteur md-doc-search)

Recherche plein-texte ciblée dans la documentation **Godot 4.7 stable**, hors-ligne, sur le corpus normalisé (fences réparées, navigation retirée : 16 238 titres exploitables contre 7 664 avant réparation).

## Emplacements

- **Moteur** : `C:/GIT/md-doc-search/target/release/md-doc-search.exe` v0.2.0+ (si absent : `cargo build --release` dans `C:/GIT/md-doc-search/`)
- **Corpus** : `C:/GIT/md-doc-search/docs/godot/godot_docs_stable_47_clean.md` (la variante brute `_47.md` reste là pour comparaison — ne pas l'utiliser)

## Commande (validée)

```bash
"C:/GIT/md-doc-search/target/release/md-doc-search.exe" \
  "C:/GIT/md-doc-search/docs/godot/godot_docs_stable_47_clean.md" \
  "Input is_action_just_pressed" --max-tokens 800
```

(La forme positionnelle `… "query" 800 3` reste valide ; `--help` liste tout.)

- **Requête** : UN seul argument, **mots-clés ANGLAIS obligatoires** (corpus indexé en anglais ; requête française = exit 1. Traduire : `character body velocity`, `navigation agent pathfinding`…).
- `--max-tokens` : budget de sortie (monter à 2500-3000 pour pages denses) ; `--top-k` : sections (défaut 3).

## Codes de sortie (contrat v0.2.0)

| Code | Sens | Réaction |
|---|---|---|
| `0` | résultats renvoyés | lire et synthétiser |
| `1` | aucun résultat | élargir : identifiant technique exact (`NomClasse methode`), retirer un mot-clé |
| `2` | erreur (corpus illisible, argument invalide) | message sur stderr — corriger l'appel |

## Règles de lecture

- Chaque bloc commence par le titre de section (`### bool is_action_just_pressed(…)`) puis une ligne `> Classe > Method Descriptions` : le fil d'Ariane situe la réponse.
- Requête idéale API : `NomClasse methode` (ex. `CharacterBody2D move_and_slide`) → la méthode exacte sort en premier.
- Un nom de classe seul (`StandardMaterial3D`) renvoie sa page de référence en premier.
- Itérer avec le nom technique exact si le premier essai est faible.

## Maintenance de l'instantané

- Le corpus est un crawl version-pinné de `https://docs.godotengine.org/en/4.7/` (pipeline complet : `docs/project/project_bible.md`).
- Après tout re-crawl : normaliser avec `py -3 C:/GIT/md-doc-search/scripts/fetch_docs.py godot` — régénère le `_clean.md`, refuse d'écrire si les fences restent cassées.

## Répondre à l'utilisateur

**Synthétiser** en français : signatures typées, paramètres, valeurs par défaut, avertissements. Ne jamais coller le markdown brut. Citer la page source si l'utilisateur veut creuser.
