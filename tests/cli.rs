//! End-to-end CLI contract: output format, exit codes, argument handling.

use std::path::Path;
use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_md-doc-search"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn demo() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("demo.md")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn results_exit_zero_and_show_breadcrumbs() {
    let (code, stdout, _stderr) = run(&[&demo(), "boolean modifier"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("--- DÉBUT DE SECTION"));
    // A `###` section under `## Options` inherits the full heading chain.
    assert!(stdout.contains("> Boolean Modifier > Options"));
}

#[test]
fn no_result_exits_one() {
    let (code, stdout, stderr) = run(&[&demo(), "zzqqxxyy"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(!stderr.is_empty());
}

#[test]
fn unreadable_file_exits_two_with_stderr() {
    let (code, stdout, stderr) = run(&["Z:/definitely/missing/corpus.md", "query"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("error"));
}

#[test]
fn zero_top_k_is_rejected_in_both_forms() {
    let (code, _stdout, stderr) = run(&[&demo(), "boolean", "--top-k", "0"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("top_k"));

    let (code, _stdout, _stderr) = run(&[&demo(), "boolean", "8000", "0"]);
    assert_eq!(code, 2);
}

#[test]
fn zero_max_tokens_is_rejected() {
    let (code, _stdout, stderr) = run(&[&demo(), "boolean", "--max-tokens", "0"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("max_tokens"));
}

#[test]
fn overflowing_max_tokens_is_rejected() {
    let (code, _stdout, stderr) =
        run(&[&demo(), "boolean", "--max-tokens", "18446744073709551615"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("max_tokens"));
}

#[test]
fn non_numeric_positional_is_rejected_instead_of_defaulted() {
    let (code, _stdout, _stderr) = run(&[&demo(), "boolean", "abc"]);
    assert_eq!(code, 2);
}

#[test]
fn positional_form_still_works() {
    let (code, stdout, _stderr) = run(&[&demo(), "boolean modifier", "8000", "2"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.matches("--- DÉBUT DE SECTION").count(), 2);
}

#[test]
fn flag_form_limits_results_and_budget() {
    let (code, stdout, _stderr) = run(&[
        &demo(),
        "boolean modifier",
        "--top-k",
        "2",
        "--max-tokens",
        "500",
    ]);
    assert_eq!(code, 0);
    assert_eq!(stdout.matches("--- DÉBUT DE SECTION").count(), 2);
    assert!(stdout.chars().count() <= 500 * 4);
}

#[test]
fn positional_and_flag_conflict_is_rejected() {
    let (code, _stdout, _stderr) = run(&[&demo(), "boolean", "8000", "3", "--top-k", "2"]);
    assert_eq!(code, 2);
}

#[test]
fn help_lists_flags() {
    let (code, stdout, _stderr) = run(&["--help"]);
    assert_eq!(code, 0);
    for flag in ["--max-tokens", "--top-k", "--fold-diacritics", "--version"] {
        assert!(stdout.contains(flag), "missing {flag} in --help");
    }
}

#[test]
fn version_prints_package_version() {
    let (code, stdout, _stderr) = run(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
}

/// A fresh directory corpus under Cargo's per-target temp dir.
fn dir_corpus(name: &str, files: &[(&str, &str)]) -> String {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (rel, content) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    root.to_string_lossy().into_owned()
}

#[test]
fn directory_corpus_cites_pages_and_isolates_them() {
    let root = dir_corpus(
        "dir_corpus_pages",
        &[
            (
                "a.md",
                "---\ntitle: \"Alpha\"\nurl: https://example.org/a.html\n---\n\n# Alpha\n\nthe zebra lives here\n",
            ),
            // No heading: must not inherit `# Alpha` from the previous page.
            ("sub/b.md", "a quokka paragraph without any heading\n"),
            (".hidden/c.md", "# Hidden\n\nquokka quokka quokka\n"),
            ("notes.txt", "# Not markdown\n\nquokka\n"),
        ],
    );

    let (code, stdout, _stderr) = run(&[&root, "zebra", "--top-k", "1"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("# Alpha"));
    assert!(stdout.contains("> Source: https://example.org/a.html"));
    assert!(stdout.contains("> File: a.md"));
    // Front matter is not searchable text.
    assert!(!stdout.contains("title:"));

    let (code, stdout, _stderr) = run(&[&root, "quokka", "--top-k", "5"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.matches("--- DÉBUT DE SECTION").count(), 1);
    assert!(stdout.contains("> File: sub/b.md"));
    assert!(!stdout.contains("> Alpha"));
    assert!(!stdout.contains("Hidden"));
    assert!(!stdout.contains("Not markdown"));
}

#[test]
fn directory_without_markdown_exits_two() {
    let root = dir_corpus("dir_corpus_empty", &[("readme.txt", "nothing")]);
    let (code, stdout, stderr) = run(&[&root, "anything"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("no .md file"));
}
