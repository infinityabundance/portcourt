//! Integration tests: drive the built `portcourt` binary against the synthetic fixture parity view and
//! assert the closure-math verdicts (the gate must pass on honest claims and fail on over-claims).

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    // CARGO_BIN_EXE_portcourt is set by cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_portcourt"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Run `portcourt <args...>` with a config written into a *unique* temp dir (parallel-test safe) alongside
/// a copy of the fixture, returning (success, stdout).
fn run_with_config(config: &str, args: &[&str]) -> (bool, String) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let id = N.fetch_add(1, Ordering::Relaxed);
    let dir = root().join(format!("target/test-tmp/{}-{id}", std::process::id()));
    let _ = std::fs::create_dir_all(dir.join("tests/fixtures"));
    std::fs::copy(root().join("tests/fixtures/parity.json"), dir.join("tests/fixtures/parity.json")).unwrap();
    std::fs::write(dir.join("pc.toml"), config).unwrap();
    // The binary's arg order is `<cmd> [operand] <config>`; append the temp config path.
    let out = Command::new(bin()).args(args_for(args)).current_dir(&dir).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Map a logical arg list into the binary's actual arg order with the temp config path appended.
fn args_for(args: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    v.push("pc.toml".to_string());
    v
}

const HONEST: &str = "parity = \"tests/fixtures/parity.json\"\n\
                      [module.\"alpha.c\"]\nclaim = \"complete\"\n\
                      [module.\"beta.c\"]\nclaim = \"partial\"\nmin_parity = 50.0\n";

const OVERCLAIM: &str = "parity = \"tests/fixtures/parity.json\"\n\
                         [module.\"beta.c\"]\nclaim = \"complete\"\n";

const OVER_PARITY: &str = "parity = \"tests/fixtures/parity.json\"\n\
                           [module.\"beta.c\"]\nclaim = \"partial\"\nmin_parity = 75.0\n";

#[test]
fn check_passes_on_honest_claims() {
    let (ok, out) = run_with_config(HONEST, &["check"]);
    assert!(ok, "honest claims should pass:\n{out}");
    assert!(out.contains("PASS"), "{out}");
}

#[test]
fn check_fails_when_partial_module_claims_complete() {
    // beta.c has a missing function, so claiming `complete` is an over-claim -> nonzero exit.
    let (ok, out) = run_with_config(OVERCLAIM, &["check"]);
    assert!(!ok, "over-claim should fail:\n{out}");
    assert!(out.contains("over-claim") || out.contains("FAIL"), "{out}");
}

#[test]
fn check_fails_when_below_min_parity() {
    // beta.c is at 50%; requiring 75% is unmet -> fail.
    let (ok, out) = run_with_config(OVER_PARITY, &["check"]);
    assert!(!ok, "below-min-parity should fail:\n{out}");
}

#[test]
fn complete_tolerates_doc_only_on_disabled_fn() {
    // alpha.c has a doc-only mention of a config-disabled fn; that must NOT break `complete`.
    let (ok, _out) = run_with_config(HONEST, &["check"]);
    assert!(ok);
}

#[test]
fn explain_reports_ported_missing_and_unknown() {
    let (ok, out) = run_with_config(HONEST, &["explain", "a_one"]);
    assert!(ok);
    assert!(out.contains("PORTED"), "{out}");
    let (_ok2, out2) = run_with_config(HONEST, &["explain", "b_two"]);
    assert!(out2.contains("MISSING"), "{out2}");
    let (ok3, out3) = run_with_config(HONEST, &["explain", "no_such_fn"]);
    assert!(!ok3, "unknown fn exits non-zero");
    assert!(out3.contains("not found"), "{out3}");
}

#[test]
fn next_lists_missing_functions() {
    let (ok, out) = run_with_config(HONEST, &["next", "beta.c"]);
    assert!(ok);
    assert!(out.contains("b_two"), "next should list the missing fn:\n{out}");
}
