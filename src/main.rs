use regex::Regex;
use std::env;
use std::fs;

/// Poids d'un mot-clé (style IDF) : les termes rares dans le manuel comptent
/// plus que les mots génériques ("modifier", "options") qui sont partout.
fn idf_weight(df: usize, n_sections: usize) -> f64 {
    ((n_sections as f64 + 1.0) / (df as f64 + 1.0)).ln().max(0.25)
}

fn search_markdown(filepath: &str, query: &str, top_k: usize, max_tokens: usize) -> String {
    let content = match fs::read_to_string(filepath) {
        Ok(c) => c,
        Err(e) => return format!("Erreur de lecture du fichier {} : {}", filepath, e),
    };

    let keywords: Vec<String> = query
        .split_whitespace()
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if keywords.is_empty() {
        return format!("Aucun résultat trouvé pour : '{}'.", query);
    }

    // Découpage en sections sur les titres H1/H2/H3 (le H3 est indispensable
    // pour les références de classes : chaque méthode "### move_and_slide"
    // devient sa propre section) ; chaque section retient sa ligne de titre ET
    // le titre H1 de la page qui la porte ("## Options" sous "# Boolean
    // Modifier" hérite ainsi du contexte de page).
    let re_split = Regex::new(r"(?m)^#{1,3}\s").unwrap();
    let mut starts: Vec<usize> = re_split.find_iter(&content).map(|m| m.start()).collect();
    starts.push(content.len());

    let mut sections: Vec<(usize, usize, String, String)> = Vec::new(); // (start, end, first_line, page_title)
    let mut page_title = String::new();
    for w in starts.windows(2) {
        let (s, e) = (w[0], w[1]);
        if content[s..e].trim().is_empty() {
            continue;
        }
        let first_line = content[s..].lines().next().unwrap_or("").to_lowercase();
        if first_line.starts_with("# ") {
            page_title = first_line.clone();
        }
        sections.push((s, e, first_line, page_title.clone()));
    }

    let n = sections.len();

    // Document frequency de chaque mot-clé (pour le poids IDF).
    let mut dfs: Vec<usize> = Vec::with_capacity(keywords.len());
    for kw in &keywords {
        let mut df = 0;
        for (s, e, _, _) in &sections {
            if content[*s..*e].to_lowercase().contains(kw.as_str()) {
                df += 1;
            }
        }
        dfs.push(df);
    }
    let weights: Vec<f64> = dfs.iter().map(|&df| idf_weight(df, n)).collect();

    // Scoring.
    let mut results: Vec<(f64, usize, String)> = Vec::new(); // (score, start, section)
    for (idx, &(s, e, ref first_line, ref page_title)) in sections.iter().enumerate() {
        let section = &content[s..e];
        let section_lower = section.to_lowercase();
        let heading_ctx = format!("{} {}", page_title, first_line);

        let mut score: f64 = 0.0;
        for (i, kw) in keywords.iter().enumerate() {
            let w = weights[i];
            // Plafond d'occurrences : une section géante ne doit pas écraser
            // le classement uniquement parce qu'elle est longue.
            let occ = section_lower.matches(kw.as_str()).count().min(20) as f64;
            score += occ * w;
            if first_line.contains(kw.as_str()) {
                score += 15.0 * w;
            }
            if page_title.contains(kw.as_str()) {
                score += 10.0 * w;
            }
        }

        // Bonus si TOUS les mots-clés sont dans les titres (page + section).
        if keywords.iter().all(|kw| heading_ctx.contains(kw.as_str())) {
            score *= 1.4;
        }

        // Bonus phrase exacte (requête complète telle quelle).
        if section_lower.contains(&query.to_lowercase()) {
            score += 15.0 * weights.iter().sum::<f64>() / keywords.len() as f64;
        }

        // Normalisation par la longueur : pénalise les sections très longues.
        score /= 1.0 + (section.len() as f64 / 6000.0).sqrt();

        if score > 0.0 {
            results.push((score, idx, section.trim().to_string()));
        }
    }

    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));

    if results.is_empty() {
        return format!("Aucun résultat trouvé pour : '{}'.", query);
    }

    let mut response = format!("Résultats de recherche pour '{}' :\n\n", query);

    // Règle standard : 1 token ≈ 4 caractères
    let max_chars = max_tokens * 4;
    let max_per_section_chars = max_chars / top_k;

    for (score, _, text) in results.into_iter().take(top_k) {
        let mut safe_text = text;

        if safe_text.len() > max_per_section_chars {
            let mut end = max_per_section_chars;
            while !safe_text.is_char_boundary(end) {
                end -= 1;
            }
            safe_text = format!(
                "{}...\n\n[... Section tronquée pour respecter le budget de tokens ...]",
                &safe_text[..end]
            );
        }

        let block = format!(
            "--- DÉBUT DE SECTION (Score: {}) ---\n{}\n--- FIN DE SECTION ---\n\n",
            score.round() as i64, safe_text
        );

        response.push_str(&block);
    }

    response
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: {} <markdown_file> <\"keywords\"> [max_tokens] [top_k]", args[0]);
        eprintln!("  max_tokens: output budget in tokens (default 8000, 1 token ≈ 4 chars)");
        eprintln!("  top_k:      number of sections to return (default 3)");
        std::process::exit(1);
    }

    let filepath = &args[1];
    let query = &args[2];

    let max_tokens = if args.len() > 3 {
        args[3].parse::<usize>().unwrap_or(8_000)
    } else {
        8_000
    };

    let top_k = if args.len() > 4 {
        args[4].parse::<usize>().unwrap_or(3)
    } else {
        3
    };

    let result = search_markdown(filepath, query, top_k, max_tokens);
    println!("{}", result);
}
