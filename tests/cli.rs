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
