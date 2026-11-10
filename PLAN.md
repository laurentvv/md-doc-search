# PLAN.md — Audit de `md-doc-search`

> Périmètre audité : `src/main.rs` (168 lignes, unique fichier source), `Cargo.toml`, `Cargo.lock`, `.gitignore`, `README.md`, `examples/demo.md`, `LICENSE`, `assets/`.
> Écosystème : Rust (edition 2024, binaire CLI, une seule dépendance `regex`).
> Vérifications effectuées : `cargo build --release`, `cargo clippy` (défaut et `pedantic`), `cargo fmt --check`, exécution du binaire sur des cas limites et sur un corpus synthétique de 10 Mo.

---

## 1. Diagnostic

**Bloquant**
- `src/main.rs:31` — Le découpage `(?m)^#{1,3}\s` ne tient pas compte des blocs de code : un `# commentaire` Python/Bash dans un bloc ```` ``` ```` devient un H1. Il coupe le bloc en deux et **remplace le titre de page hérité** de toutes les sections suivantes (vérifié). C'est critique pour les corpus d'API (Blender Python, Godot). Le texte placé avant le premier titre n'est jamais indexé (vérifié).
- `src/main.rs:114` — `top_k=0` déclenche une panique par division par zéro (exit 101). `src/main.rs:12-15` : une erreur de lecture est écrite sur **stdout avec le code de sortie 0**, donc un agent ne peut pas distinguer une erreur d'un résultat.
- Le dépôt n'a ni test (0 `#[test]`) ni CI (pas de `.github/`). `cargo fmt --check` échoue déjà (3 écarts), et le badge « windows | linux | macos » du `README.md` n'est vérifié nulle part.

**Important**
- `src/main.rs:11-139` — Tout est dans une seule fonction de 130 lignes : parsing, IDF, scoring et rendu. Elle utilise un tuple `(usize, usize, String, String)`, renvoie un `String` (erreurs comprises) et contient 7 constantes magiques non nommées.
- Pertinence : le budget est réparti uniformément (`max_chars / top_k`). Une section réduite à son titre (`## Options` dans `examples/demo.md`) occupe la 2ᵉ place (vérifié). Le bonus de phrase exacte dépend des espaces multiples, et aucun facteur de couverture des mots-clés n'est appliqué.
- Performance : le corpus est passé en minuscules K+1 fois (`src/main.rs:56` et `:68`). Sur 10 Mo, la requête passe de 38 ms (1 mot-clé) à 134 ms (12 mots-clés).
- Sécurité côté LLM : un corpus crawlé peut imiter les délimiteurs `--- FIN DE SECTION ---` (vérifié). Il n'y a aucune limite de taille de fichier.

**Mineur**
- Les messages de sortie sont en français alors que l'usage et le `README.md` sont en anglais. La CLI est positionnelle, sans `--help`/`--version`, et remplace silencieusement un argument invalide par la valeur par défaut. `Cargo.toml` n'a ni `rust-version` ni métadonnées. `.gitignore` ignore `/docs`.

---

## 2. Checklist de Refactoring

### Parsing Markdown (exactitude du découpage)
- [ ] [P0] `src/main.rs:31-47` — Remplacer le découpage par regex par un scanner ligne par ligne qui ignore les lignes situées dans des blocs de code délimités (```` ``` ```` et `~~~`, avec gestion de la longueur de la clôture).
- [ ] [P0] `src/main.rs:32-33` — Indexer le préambule (texte avant le premier titre) comme une section à part entière, au lieu de le perdre en démarrant `starts` au premier titre.
- [ ] [P1] `src/main.rs:31` — Reconnaître les titres précédés de 0 à 3 espaces (CommonMark) et les titres Setext (`Titre\n====` → H1, `----` → H2) ; attention à ne pas confondre un `---` isolé (règle horizontale) avec un soulignement Setext.
- [ ] [P1] `src/main.rs:42-46` — Maintenir une pile de titres H1/H2/H3 (fil d'Ariane) au lieu du seul `page_title`, afin qu'une section H3 hérite aussi de son H2 (`### Solver` sous `## Options`).
- [ ] [P1] `src/main.rs:37-47` — Rattacher les sections sans corps (titre seul, ex. `## Options`) à leur première sous-section, ou les exclure des résultats, pour qu'elles n'occupent plus un emplacement du top-k.
- [ ] [P2] `README.md` — Documenter explicitement que `####` et au-delà ne créent pas de section (comportement voulu mais implicite).

### Gestion des erreurs et contrat CLI
- [ ] [P0] `src/main.rs:113-114` — Refuser `top_k = 0` (et `max_tokens = 0`) avec un message clair sur stderr et un code de sortie 2, au lieu de paniquer ou de renvoyer des sections vides.
- [ ] [P0] `src/main.rs:11-15`, `:166-167` — Faire renvoyer `Result<Vec<Hit>, SearchError>` à `search_markdown`. En cas d'erreur, écrire sur **stderr** et sortir avec un code non nul (convention `grep` : 0 = résultats, 1 = aucun résultat, 2 = erreur).
- [ ] [P1] `src/main.rs:154-164` — Ne plus remplacer silencieusement une valeur non numérique (`unwrap_or(8_000)`) : la rejeter avec un message d'usage.
- [ ] [P1] `src/main.rs:141-164` — Migrer l'analyse des arguments vers `clap` (derive, `default-features = false` + `std,derive,help,usage,error-context`) : `--max-tokens`, `--top-k`, `--help`, `--version`. Garder la compatibilité avec la forme positionnelle actuelle utilisée par les `SKILL.md` existants.
- [ ] [P1] `src/main.rs:104` — Remplacer `partial_cmp(...).unwrap()` par `f64::total_cmp` pour supprimer ce chemin de panique.
- [ ] [P2] `src/main.rs:145` — Ne plus indexer `args[0]` sans vérification (un `argv` vide provoque une panique) : utiliser `env!("CARGO_PKG_NAME")` dans l'usage.

### Structure des modules et typage
- [ ] [P1] `src/main.rs` → `src/lib.rs` + `src/main.rs` — Extraire une bibliothèque : `parse.rs` (sections), `rank.rs` (IDF + scoring), `render.rs` (texte/budget). `main.rs` ne garde que la CLI.
- [ ] [P1] `src/main.rs:35` — Remplacer le tuple `(usize, usize, String, String)` par `struct Section { range: Range<usize>, level: u8, heading: String, breadcrumb: Vec<usize> }`, et `(f64, usize, String)` par `struct Hit { score: f64, section_idx: usize }`.
- [ ] [P1] `src/main.rs:8,76,79,82,88,93,97` — Nommer les constantes (`IDF_FLOOR = 0.25`, `OCC_CAP = 20`, `HEADING_BONUS = 15.0`, `PAGE_BONUS = 10.0`, `ALL_IN_HEADINGS_MULT = 1.4`, `PHRASE_BONUS = 15.0`, `LEN_NORM_BYTES = 6000.0`) et les regrouper dans une `struct RankingParams` avec `Default`, ce qui permettra de les régler et de les tester.
- [ ] [P1] `src/main.rs:31` — Si `regex` reste utilisé, compiler le motif une seule fois via `std::sync::LazyLock` (utile dès qu'un mode serveur réutilise la fonction).
- [ ] [P2] `src/main.rs:5-6,26-30,51,64,…` — Uniformiser la langue des commentaires et rustdoc (l'anglais, cohérent avec le README), et documenter `search_markdown` / l'API publique de la lib.

### Pertinence du classement
- [ ] [P1] `src/main.rs:113-136` — Remplacer le budget uniforme `max_chars / top_k` par une allocation gloutonne : le budget non utilisé par les sections courtes est redistribué aux sections longues.
- [ ] [P1] `src/main.rs:119-127` — Tronquer à une frontière de ligne et refermer un bloc de code resté ouvert après la coupe. Compter en caractères (`chars().count()` ou `char_indices`) plutôt qu'en octets, car le « 1 token ≈ 4 chars » est aujourd'hui appliqué à des octets.
- [ ] [P1] `src/main.rs:92` — Construire la phrase exacte à partir des `keywords` normalisés (`join(" ")`) plutôt qu'à partir de `query`. Ne pas appliquer ce bonus aux requêtes d'un seul mot-clé (double comptage avec les occurrences).
- [ ] [P1] `src/main.rs:71-89` — Ajouter un facteur de couverture (`score *= matched_kw / total_kw`, façon Lucene classique), pour qu'une section contenant un seul terme répété ne dépasse plus une section qui les contient tous.
- [ ] [P2] `src/main.rs:76` — Ajouter un bonus de correspondance sur mot entier (frontières `\b` ou `_`), pour que `is` ou `input` ne se contentent pas d'une correspondance dans `this` ou `inputs`.
- [ ] [P2] `src/main.rs:17-21` — Ajouter une option de repli des diacritiques (`é` → `e`) pour les corpus non anglais.

### Performance
- [ ] [P1] `src/main.rs:51-68` — Passer chaque section en minuscules **une seule fois** (`Vec<String>` mis en cache) et réutiliser ce texte pour le calcul des DF et pour le scoring. Aujourd'hui, le corpus est re-minusculisé K+1 fois (38 → 134 ms sur 10 Mo de 1 à 12 mots-clés).
- [ ] [P1] `src/main.rs:92`, `:69` — Sortir de la boucle `query.to_lowercase()` et le `format!` de `heading_ctx`, recalculés aujourd'hui pour chaque section.
- [ ] [P1] `src/main.rs:100`, `:104` — Ne plus allouer `section.trim().to_string()` pour chaque section trouvée : conserver l'indice, sélectionner le top-k via `select_nth_unstable_by` ou un `BinaryHeap` borné, et ne matérialiser que les `top_k` textes.
- [ ] [P2] `src/main.rs:44-46` — Éviter le `page_title.clone()` pour chaque section (stocker un indice vers la section H1).
- [ ] [P2] `Cargo.toml` — Ajouter un `[profile.release]` adapté à un binaire distribué : `lto = "thin"`, `codegen-units = 1`, `strip = true`.
- [ ] [P2] `benches/` — Ajouter un benchmark (`criterion` ou `divan`) sur un corpus synthétique de 10 Mo pour prévenir les régressions de la cible « ~40 ms » annoncée dans le `README.md`.

### Sécurité
- [ ] [P1] `src/main.rs:130-133` — Rendre les délimiteurs de section impossibles à falsifier par le corpus : nonce aléatoire par exécution (`--- SECTION 3f9a… ---`) ou échappement des lignes qui reproduisent le délimiteur. Le contenu crawlé sur le web n'est pas fiable (risque d'injection de prompt envers l'agent).
- [ ] [P1] `src/main.rs:12` — Vérifier `fs::metadata(path)?.len()` avant la lecture et refuser au-delà d'un plafond configurable (`--max-file-size`, par défaut 256 Mo). Rejeter aussi ce qui n'est pas un fichier régulier (`/dev/zero`, FIFO), qui provoque un blocage ou un épuisement mémoire.
- [ ] [P1] `README.md` (section « Grounding your AI agent ») — Signaler que l'exemple `SKILL.md` insère une requête fournie par l'agent entre guillemets doubles dans une commande shell (risque d'injection via `"` / `$()`). Recommander les guillemets simples ou, mieux, le mode MCP (fonctionnalité 1).
- [ ] [P2] `src/main.rs:135` — Filtrer les séquences d'échappement ANSI/caractères de contrôle du corpus lorsque stdout est un TTY.

### Observabilité
- [ ] [P1] `src/main.rs:66-102` — Ajouter `--explain`, qui affiche sur stderr la décomposition du score de chaque résultat (poids IDF, occurrences, bonus titre/page/phrase, normalisation). C'est indispensable pour régler `RankingParams` sans deviner.
- [ ] [P2] `src/main.rs:141-168` — Ajouter `-v/--verbose` : nombre de sections, temps de lecture/parsing/scoring et taille du corpus sur stderr (sans polluer la sortie consommée par l'agent).

### Tests
- [ ] [P0] `src/parse.rs` (tests unitaires) — Couvrir le découpage : blocs de code contenant `# commentaire`, préambule, titres Setext, titres indentés, fichiers en CRLF, fichier vide, fichier sans titre.
- [ ] [P0] `tests/cli.rs` (`assert_cmd` + `predicates`) — Tester les codes de sortie : fichier absent (≠ 0, stderr), `top_k=0`, `max_tokens=0`, argument non numérique, aucun résultat, arguments manquants.
- [ ] [P1] `tests/ranking.rs` — Tests d'ordre de classement sur `examples/demo.md` : `boolean modifier` → `# Boolean Modifier` en 1ʳᵉ position, `volume scatter anisotropy`, ordre déterministe à score égal. Ajouter un mini-corpus de régression de type API (méthode H3 attendue au rang 1).
- [ ] [P1] `src/render.rs` (tests unitaires) — Troncature sur une frontière UTF-8 (`é`, emoji, CJK), marqueur de troncature présent, respect du budget global.
- [ ] [P1] `src/rank.rs` (tests unitaires) — `idf_weight` (df = 0, df = n, plancher 0,25), plafond d'occurrences, normalisation par la longueur.
- [ ] [P2] `tests/` (`proptest`) — Propriété « aucune panique » sur des entrées Markdown et des requêtes arbitraires, avec `top_k` et `max_tokens` quelconques.
- [ ] [P2] `tests/snapshots/` (`insta`) — Snapshots de la sortie texte (puis JSON) pour détecter tout changement involontaire de format.

### Dépendances et outillage
- [ ] [P1] `.github/workflows/ci.yml` — Créer une CI avec une matrice `ubuntu-latest` / `windows-latest` / `macos-latest` : `cargo fmt --check` (qui échoue aujourd'hui), `cargo clippy --all-targets -- -D warnings`, `cargo test`, plus un job MSRV 1.85.
- [ ] [P1] `src/main.rs` — Exécuter `cargo fmt` pour corriger les 3 écarts actuels (`:8`, `:130-133`, `:145`).
- [ ] [P1] `Cargo.toml` — Déclarer `rust-version = "1.85"` (exigence du README et de l'edition 2024, aujourd'hui non vérifiée par Cargo).
- [ ] [P1] `.github/workflows/ci.yml` — Ajouter `cargo audit` (ou `cargo deny check advisories licenses`). `regex 1.13.1` est la dernière version et le lockfile est versionné (bonne pratique pour un binaire), mais aucun contrôle automatisé n'existe.
- [ ] [P2] `.github/dependabot.yml` — Mises à jour hebdomadaires de l'écosystème `cargo` et des actions GitHub.
- [ ] [P2] `.github/workflows/release.yml` — Publier des binaires précompilés (Windows/Linux/macOS) sur tag, via `cargo-dist` : l'exemple `SKILL.md` suppose un `.exe` compilé localement.
- [ ] [P2] `Cargo.toml` — Réévaluer `regex` : une fois le scanner ligne par ligne en place, le seul motif utilisé disparaît. On pourra alors supprimer la dépendance, ou la garder uniquement pour la fonctionnalité 3.

### Documentation
- [ ] [P1] `src/main.rs:14,23,107,110,125,131` et `README.md:181` — Passer les messages de sortie en anglais, cohérents avec l'usage (`src/main.rs:145-147`) et le README, ou ajouter `--lang fr|en`. Mettre à jour l'exemple de sortie `README.md:18-25`.
- [ ] [P1] `README.md` (« How ranking works ») — Documenter les éléments absents : valeur du bonus de phrase exacte, constante de normalisation (6000 octets), sémantique OU des mots-clés, correspondance par sous-chaîne (pas par mot entier), comptage du budget en octets.
- [ ] [P1] `README.md` (« Limitations ») — Ajouter les limites actuelles tant qu'elles ne sont pas corrigées : `#` dans les blocs de code, préambule ignoré, titres Setext non reconnus.
- [ ] [P2] `Cargo.toml` — Ajouter `description`, `license = "MIT"`, `repository`, `readme`, `keywords`, `categories`, préalables à un `cargo install md-doc-search` depuis crates.io.
- [ ] [P2] `.gitignore:2` — Renommer l'exclusion `/docs` (probablement destinée aux corpus locaux) en `/corpora`, pour ne pas bloquer un futur dossier `docs/` de documentation du projet.
- [ ] [P2] `README.md:9` — Corriger « ~150-line » (168 lignes aujourd'hui, davantage après refactoring) ou retirer le chiffre.

---

## 3. Checklist de Nouvelles Fonctionnalités

### Fonctionnalité 1 — Mode serveur MCP (`md-doc-search mcp`)

L'agent appelle un outil typé au lieu de construire une commande shell. Chaque corpus est parsé une seule fois puis gardé en mémoire.

- [ ] Prérequis : terminer l'extraction de `src/lib.rs` avec `Corpus::load(path) -> Result<Corpus>`, qui précalcule les sections, le texte en minuscules et le fil d'Ariane.
- [ ] Ajouter la sous-commande `md-doc-search mcp --corpus godot=/path/godot_47.md --corpus blender=/path/Blender52.md` (transport stdio).
- [ ] Exposer l'outil `search_docs { corpus, query, top_k?, max_tokens? }` et l'outil `list_corpora` (nom, chemin, taille, nombre de sections, date de modification), avec des descriptions d'outils qui imposent des mots-clés dans la langue du corpus.
- [ ] Restreindre l'accès aux corpus déclarés au démarrage : l'agent ne peut jamais fournir de chemin arbitraire.
- [ ] Recharger un corpus lorsque sa date de modification change (vérification à chaque appel, sans watcher).
- [ ] Test d'intégration : lancer le binaire, envoyer `initialize`, `tools/list` puis `tools/call` sur stdin, et vérifier la réponse sur stdout.
- [ ] Documenter le bloc `mcpServers` dans `README.md`, en miroir de la configuration `crawl4ai-mcp-llm` déjà présentée.

**Approche technique**
Utiliser la crate `rmcp` (SDK Rust officiel de MCP) derrière une feature Cargo `mcp`, non activée par défaut, pour que le binaire CLI de base reste léger (`tokio` n'entre que dans ce mode). Le serveur possède un `HashMap<String, Corpus>` chargé au démarrage. Chaque `tools/call` réutilise `rank::search(&corpus, &query, &params)` de la lib : le code de scoring est le même qu'en CLI, sans duplication. Le gain est double. La latence tombe sous la milliseconde après le chargement, puisque le parsing et la mise en minuscules sont payés une fois. Et l'injection shell disparaît, car la requête circule en JSON. Écarté : un serveur HTTP, qui ajoute une surface réseau inutile pour un outil local et hors ligne. Écarté aussi : un JSON-RPC écrit à la main avec `serde_json`, possible mais qui obligerait à suivre soi-même les évolutions du protocole MCP.

### Fonctionnalité 2 — Sortie structurée JSON avec citations (`--format json`)

Pour chaque résultat, l'agent reçoit le fil d'Ariane, les lignes source et l'URL canonique, ce qui lui permet de citer précisément et de rouvrir la page.

- [ ] Étendre le parseur pour conserver, par section, le fil d'Ariane complet (`["Boolean Modifier", "Options", "Solver"]`) et la plage de lignes (`line_start`, `line_end`).
- [ ] Extraire `source_url` depuis la ligne `> Source: <url>` la plus proche en amont dans la page (convention déjà recommandée par le `README.md` et produite par `crawl4ai-mcp-llm`).
- [ ] Ajouter `--format text|json` (texte par défaut, rétrocompatible). En JSON, émettre `{ query, corpus, results: [{ rank, score, breadcrumb, line_start, line_end, source_url, truncated, text }] }`.
- [ ] En mode texte, afficher le fil d'Ariane et `fichier:ligne` dans l'en-tête de section (`--- SECTION 1 · Boolean Modifier › Options · demo.md:13 ---`).
- [ ] Ajouter `--scope section|page` pour renvoyer au besoin la page H1 entière qui contient la meilleure section.
- [ ] Snapshots `insta` pour les deux formats et test de validité JSON sur un corpus contenant guillemets, retours chariot et caractères de contrôle.

**Approche technique**
Ajouter `serde` (derive) et `serde_json`. La `struct Hit` issue du refactoring devient `#[derive(Serialize)]`, et le rendu texte comme le rendu JSON deviennent deux fonctions de `render.rs` alimentées par les mêmes données. Les numéros de ligne s'obtiennent en une passe : on précalcule les positions des `\n` avec `memchr`, déjà présent en dépendance transitive, puis on fait une recherche dichotomique sur l'offset de début de section. Le JSON règle aussi le problème des délimiteurs falsifiables : le contenu est échappé et le parseur de l'agent ne peut pas être trompé. La sortie JSON est enfin le format naturel des réponses de l'outil MCP (fonctionnalité 1). Écarté : YAML ou XML, moins bien pris en charge par les frameworks d'agents et plus coûteux en tokens.

### Fonctionnalité 3 — Préparation et contrôle qualité des corpus (`md-doc-search lint` / `prep`)

Le but est d'industrialiser les recettes de nettoyage aujourd'hui décrites à la main dans la section « Preparing corpora » du `README.md`.

- [ ] `md-doc-search lint corpus.md` : produire un rapport (sections totales, sections sans corps, sections > 20 Ko, titres qui ne sont qu'une URL, `**method**(` non promus en H3, `#` dans des blocs de code, lignes répétées dans plus de X % des pages).
- [ ] `md-doc-search prep in.md -o out.md` avec des options activables séparément : `--promote-bold-methods` (recette B), `--demote-link-headings` (recette C), `--strip-repeated-lines <ratio>` (recette A, détection automatique du boilerplate de navigation).
- [ ] Accepter plusieurs fichiers d'entrée (`prep crawl/*.md -o godot_47.md`) pour concaténer une sortie de crawl en un corpus unique figé sur une version, en préservant les lignes `> Source:`.
- [ ] Ajouter `--dry-run`, qui affiche seulement les statistiques de transformation (ex. « 6 391 méthodes promues, 1 590 blocs retirés »).
- [ ] Tests d'idempotence (`prep(prep(x)) == prep(x)`) et tests qui vérifient qu'aucune transformation n'est appliquée à l'intérieur d'un bloc de code.

**Approche technique**
Réutiliser le scanner ligne par ligne conscient des blocs de code, introduit dans `src/parse.rs` pour la recherche. Ainsi, `lint`, `prep` et `search` partagent une seule définition de « ce qui est un titre », et le corpus préparé est garanti conforme à ce que le moteur découpe. Les recettes B et C reprennent les motifs `regex` déjà documentés dans le `README.md` : la dépendance existe déjà, aucun ajout n'est nécessaire. La détection du boilerplate compte, dans un `HashMap<&str, usize>`, le nombre de pages H1 où apparaît chaque ligne normalisée. Le traitement se fait en flux (`BufRead` / `BufWriter`) pour absorber des crawls de plusieurs centaines de Mo. Les sous-commandes s'ajoutent naturellement si la CLI passe à `clap` (checklist « Gestion des erreurs et contrat CLI »). Écarté : laisser ces recettes à des scripts Python externes, qui ne seraient ni versionnés, ni testés, ni alignés avec le parseur.
