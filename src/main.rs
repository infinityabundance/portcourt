//! # portcourt — a general C→Rust port evidence court
//!
//! A faithful C→Rust port lives or dies by one question: *is the claim backed by the evidence?* portcourt
//! makes that question mechanical. It consumes the parity view a port already produces (the typed C-symbol
//! ↔ Rust-`fn` join) and a small declaration of what each module *claims*, then enforces **closure math**:
//! a module that claims to be `complete` must have zero missing functions and zero doc-only false hits, and
//! every court it `requires` must actually be sealed — otherwise `portcourt check` fails the build.
//!
//! Four subcommands:
//! - `portcourt check [config.toml]` — the gate: verify every module's claim against the evidence.
//! - `portcourt explain <fn> [config.toml]` — the per-function status: ported? missing? a doc-only hit?
//! - `portcourt next [file] [config.toml]` — the porting driver: what is still missing.
//! - `portcourt report [config.toml]` — the parity table across all modules.
//!
//! It is deliberately general: nothing here is specific to any one port. Point the config at your parity
//! JSON and claim ladder and it works.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// ---------------------------------------------------------------------------------------------------------
// Evidence schema — the parity view (one entry per C source file, each with its functions + port status)
// and the claim ladder (the sealed courts). These are exactly the artifacts a faithful port already emits.
// ---------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct FileParity {
    file: String,
    #[serde(default)]
    fns: Vec<FnEntry>,
}

#[derive(Deserialize)]
struct FnEntry {
    function: String,
    /// `active` / `inactive` / `test` (a real Rust `fn`), `missing` (no port), `doc_only` (name only in a
    /// comment — a false hit, not a port), or `disabled` (`#if 0` in the C, not a port target).
    rust_status: String,
    /// `compiled` vs `config_disabled` (a `#if 0` / unselected C function that is not a port target). A
    /// doc-only mention of a non-compiled function is benign — only a compiled function must be ported.
    #[serde(default)]
    preprocessor_status: Option<String>,
    #[serde(default)]
    rust_module: Option<String>,
}

impl FnEntry {
    fn is_compiled(&self) -> bool {
        // Treat an unspecified preprocessor status as compiled (the conservative default).
        self.preprocessor_status.as_deref().map(|s| s == "compiled").unwrap_or(true)
    }
}

#[derive(Deserialize, Default)]
struct ClaimLadder {
    #[serde(default)]
    courts: Vec<Court>,
}

#[derive(Deserialize)]
struct Court {
    id: String,
    #[serde(default)]
    sealed_version: Option<String>,
    #[serde(default)]
    proven: Option<String>,
}

// ---------------------------------------------------------------------------------------------------------
// Config — the declaration of intent. Every module names what it claims; portcourt checks it against fact.
// ---------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct Config {
    /// Path to the parity JSON (array of per-file entries), relative to the config file.
    parity: String,
    /// Optional path to the claim-ladder JSON (for `requires`), relative to the config file.
    #[serde(default)]
    courts: Option<String>,
    /// Per-module claims, keyed by the C source file name (e.g. `"fileio.c"`).
    #[serde(default)]
    module: BTreeMap<String, ModuleClaim>,
}

#[derive(Deserialize, Default)]
struct ModuleClaim {
    /// `"complete"` (every function ported, no doc-only) or `"partial"`.
    #[serde(default)]
    claim: Option<String>,
    /// Minimum required active-parity percentage (`0.0`..=`100.0`).
    #[serde(default)]
    min_parity: Option<f64>,
    /// Courts that must be sealed for this module's claim to hold.
    #[serde(default)]
    requires: Vec<String>,
}

/// The tallied port status of one module.
struct Tally {
    ported: u32,
    missing: u32,
    doc_only: u32,
    /// Doc-only hits on *compiled* C functions — the ones that actually matter (a doc-only on a
    /// `#if 0`/config-disabled function is benign).
    doc_only_live: u32,
    disabled: u32,
}

impl Tally {
    fn of(fp: &FileParity) -> Tally {
        let mut t = Tally { ported: 0, missing: 0, doc_only: 0, doc_only_live: 0, disabled: 0 };
        for f in &fp.fns {
            match f.rust_status.as_str() {
                "active" | "inactive" | "test" => t.ported += 1,
                "missing" => t.missing += 1,
                "doc_only" => {
                    t.doc_only += 1;
                    if f.is_compiled() {
                        t.doc_only_live += 1;
                    }
                }
                "disabled" => t.disabled += 1,
                _ => {}
            }
        }
        t
    }
    /// Active parity over the portable surface (excludes `#if 0`-disabled functions and benign doc-only
    /// mentions of non-compiled functions).
    fn pct(&self) -> f64 {
        let portable = self.ported + self.missing + self.doc_only_live;
        if portable == 0 {
            100.0
        } else {
            100.0 * self.ported as f64 / portable as f64
        }
    }
    /// "complete" = every compiled function has a real Rust `fn` and there are no doc-only false hits on a
    /// compiled function.
    fn is_complete(&self) -> bool {
        self.missing == 0 && self.doc_only_live == 0
    }
}

// ---------------------------------------------------------------------------------------------------------

fn load_config(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading config {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parsing config {}: {e}", path.display()))
}

fn load_parity(cfg: &Config, base: &Path) -> Result<Vec<FileParity>, String> {
    let p = base.join(&cfg.parity);
    let text = std::fs::read_to_string(&p).map_err(|e| format!("reading parity {}: {e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parsing parity {}: {e}", p.display()))
}

fn load_courts(cfg: &Config, base: &Path) -> Result<ClaimLadder, String> {
    match &cfg.courts {
        None => Ok(ClaimLadder::default()),
        Some(rel) => {
            let p = base.join(rel);
            let text = std::fs::read_to_string(&p).map_err(|e| format!("reading courts {}: {e}", p.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("parsing courts {}: {e}", p.display()))
        }
    }
}

/// The directory the config lives in — all other paths resolve relative to it.
fn config_base(path: &Path) -> PathBuf {
    path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------------------------------------
// check — the closure-math gate. A module's claim must be backed by its evidence, or this fails.
// ---------------------------------------------------------------------------------------------------------

fn cmd_check(config_path: &Path) -> ExitCode {
    let cfg = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let base = config_base(config_path);
    let parity = match load_parity(&cfg, &base) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let ladder = match load_courts(&cfg, &base) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    // A court is "sealed" iff the claim ladder records a `sealed_version` for it. We pre-index the sealed
    // court ids into a set so the per-module `requires` loop below is an O(1) membership test rather than a
    // linear scan of the ladder for every required court.
    let sealed: std::collections::BTreeSet<&str> = ladder
        .courts
        .iter()
        .filter(|c| c.sealed_version.is_some())
        .map(|c| c.id.as_str())
        .collect();
    // Index the parity view by C file name so each declared module resolves to its evidence in O(1).
    let by_file: BTreeMap<&str, &FileParity> = parity.iter().map(|f| (f.file.as_str(), f)).collect();

    // `violations` accumulates every way a claim out-ran its evidence; a non-empty list is a failed gate.
    // We collect them all (rather than bailing on the first) so one run reports every problem at once.
    let mut violations: Vec<String> = Vec::new();
    let mut checked = 0usize;

    println!("portcourt check — closure math over {} declared module(s)\n", cfg.module.len());
    // BTreeMap iteration is sorted by file name, so the report is stable/deterministic run-to-run.
    for (file, claim) in &cfg.module {
        // (0) The module must actually exist in the parity view. Declaring a claim for a file the evidence
        //     does not even contain is itself an over-claim — you cannot vouch for what you cannot see.
        let fp = match by_file.get(file.as_str()) {
            Some(f) => *f,
            None => {
                violations.push(format!("module `{file}` is declared but absent from the parity view"));
                continue;
            }
        };
        checked += 1;
        // Tally the four port states for this module once; every rule below reads from this single count.
        let t = Tally::of(fp);
        let pct = t.pct();
        // `module_ok` flips to false on the first broken rule; `notes` records the human-readable reasons so
        // they can be printed inline under the module AND lifted into the global `violations` list.
        let mut module_ok = true;
        let mut notes: Vec<String> = Vec::new();

        // (1) The completeness rule. A module that says `claim = "complete"` is asserting that EVERY
        //     compiled C function has a real Rust `fn` and that no compiled function is merely *named* in a
        //     comment (a doc-only false hit inflates the apparent count without being a port). Note we use
        //     `doc_only_live` (doc-only on *compiled* functions) — a doc-only mention of a `#if 0`/
        //     config-disabled function is benign and must not break a true 100%.
        if claim.claim.as_deref() == Some("complete") {
            if !t.is_complete() {
                module_ok = false;
                notes.push(format!(
                    "claims `complete` but has {} missing + {} doc-only on compiled functions (over-claim)",
                    t.missing, t.doc_only_live
                ));
            }
        }
        // (2) The parity-floor rule. An optional `min_parity` lets a *partial* module still assert a lower
        //     bound ("at least 60% ported"). The `1e-9` slack absorbs floating-point rounding so a module
        //     sitting exactly on the floor is not failed by a representation artefact.
        if let Some(min) = claim.min_parity {
            if pct + 1e-9 < min {
                module_ok = false;
                notes.push(format!("parity {pct:.1}% is below the required {min:.1}%"));
            }
        }
        // (3) The dependency rule — the heart of "closure math". A module may declare the courts its claim
        //     RESTS ON (`requires`). Each such court must be sealed; if a court is still open, the claim is
        //     standing on evidence that does not yet exist, so the gate fails. This is what stops a port
        //     from quoting a green lower-court result as proof of a higher-court claim.
        for court in &claim.requires {
            if !sealed.contains(court.as_str()) {
                module_ok = false;
                notes.push(format!("requires court `{court}` which is not sealed"));
            }
        }

        let mark = if module_ok { "ok  " } else { "FAIL" };
        println!(
            "  [{mark}] {file:<14} {pct:5.1}%  ported {:>3}  missing {:>3}  doc-only {:>2}  ({} required court(s))",
            t.ported,
            t.missing,
            t.doc_only,
            claim.requires.len()
        );
        for n in &notes {
            println!("         - {n}");
        }
        if !module_ok {
            for n in notes {
                violations.push(format!("{file}: {n}"));
            }
        }
    }

    // Files present in the parity view but undeclared in the config — not a failure, but surfaced so a
    // port can't quietly leave a module ungoverned.
    let undeclared: Vec<&str> = by_file
        .keys()
        .filter(|f| !cfg.module.contains_key(**f))
        .copied()
        .collect();
    if !undeclared.is_empty() {
        println!("\n  note: {} module(s) in the parity view are undeclared in the config:", undeclared.len());
        println!("        {}", undeclared.join(", "));
    }

    println!();
    if violations.is_empty() {
        println!("PORTCOURT: PASS — {checked} module claim(s) all backed by evidence.");
        ExitCode::SUCCESS
    } else {
        println!("PORTCOURT: FAIL — {} over-claim(s):", violations.len());
        for v in &violations {
            println!("  ✗ {v}");
        }
        ExitCode::FAILURE
    }
}

// ---------------------------------------------------------------------------------------------------------
// explain — the per-function status across the whole port.
// ---------------------------------------------------------------------------------------------------------

fn cmd_explain(name: &str, config_path: &Path) -> ExitCode {
    let cfg = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let base = config_base(config_path);
    let parity = match load_parity(&cfg, &base) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let ladder = load_courts(&cfg, &base).unwrap_or_default();

    // Scan every module for a C function of this exact name. A name can in principle appear in more than
    // one translation unit (e.g. a `static` helper reused across files), so we collect ALL hits and report
    // each, rather than assuming a single match.
    let mut hits: Vec<(&str, &FnEntry)> = Vec::new();
    for fp in &parity {
        for f in &fp.fns {
            if f.function == name {
                hits.push((fp.file.as_str(), f));
            }
        }
    }
    // A name absent from the parity view is not a C function in any covered module — exit non-zero so
    // `explain` can be used as a guard ("does this symbol exist?") in a script.
    if hits.is_empty() {
        println!("portcourt explain `{name}`: not found in the parity view (not a C function in any covered module).");
        return ExitCode::FAILURE;
    }
    for (file, f) in &hits {
        // Translate the raw parity status into a human verdict. The four states are mutually exclusive per
        // function: a real Rust `fn` (PORTED), no port at all (MISSING), a name that only shows up in a
        // comment (DOC-ONLY — counted nowhere as a port, and a false hit to be reworded), or a C function
        // behind `#if 0` that was never a port target in the first place (DISABLED).
        let status = match f.rust_status.as_str() {
            "active" | "inactive" | "test" => "PORTED",
            "missing" => "MISSING (no Rust fn)",
            "doc_only" => "DOC-ONLY (name only in a comment — a false hit, not a port)",
            "disabled" => "DISABLED (#if 0 in the C — not a port target)",
            other => other,
        };
        println!("portcourt explain `{name}`");
        println!("  file:   {file}");
        println!("  status: {status}");
        if let Some(m) = &f.rust_module {
            println!("  module: {m} (Rust)");
        }
        // Surface any court that VOUCHES for this function — i.e. whose `proven` prose names it. This is a
        // deliberately loose substring match (a court describes its evidence in prose, not as a symbol
        // list), so it answers "what, if anything, certifies this function?" at a glance. A function can be
        // ported yet uncertified (no court names it); that is exactly the gap a reviewer wants to see.
        let refs: Vec<&str> = ladder
            .courts
            .iter()
            .filter(|c| c.proven.as_deref().map(|p| p.contains(name)).unwrap_or(false))
            .map(|c| c.id.as_str())
            .collect();
        if !refs.is_empty() {
            println!("  courts: {}", refs.join(", "));
        }
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------------------------------------
// next — the porting driver: which functions are still missing.
// ---------------------------------------------------------------------------------------------------------

fn cmd_next(file_filter: Option<&str>, config_path: &Path) -> ExitCode {
    let cfg = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let base = config_base(config_path);
    let parity = match load_parity(&cfg, &base) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `any` tracks whether we printed a single work item; if nothing is missing anywhere (or in the chosen
    // file) we say so explicitly rather than printing an empty list, so the absence of output is never
    // ambiguous ("did it run?").
    let mut any = false;
    for fp in &parity {
        // An optional file filter narrows the worklist to one module; without it, every module is listed.
        if let Some(filt) = file_filter {
            if fp.file != filt {
                continue;
            }
        }
        // Two distinct kinds of work: `missing` functions need a real port; `doc_only` hits need the comment
        // that names them reworded (they are false hits inflating nothing, but they muddy the parity view).
        let missing: Vec<&str> = fp
            .fns
            .iter()
            .filter(|f| f.rust_status == "missing")
            .map(|f| f.function.as_str())
            .collect();
        let doc_only: Vec<&str> = fp
            .fns
            .iter()
            .filter(|f| f.rust_status == "doc_only")
            .map(|f| f.function.as_str())
            .collect();
        if missing.is_empty() && doc_only.is_empty() {
            continue;
        }
        any = true;
        println!("{} — {} missing, {} doc-only", fp.file, missing.len(), doc_only.len());
        if !missing.is_empty() {
            println!("  missing : {}", missing.join(", "));
        }
        if !doc_only.is_empty() {
            println!("  doc-only: {} (reword the comment that names these)", doc_only.join(", "));
        }
    }
    if !any {
        println!("portcourt next: nothing missing{}.", file_filter.map(|f| format!(" in {f}")).unwrap_or_default());
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------------------------------------
// report — the parity table across all modules.
// ---------------------------------------------------------------------------------------------------------

fn cmd_report(config_path: &Path) -> ExitCode {
    let cfg = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    let base = config_base(config_path);
    let parity = match load_parity(&cfg, &base) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("portcourt: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{:<16} {:>7}  {:>6} {:>7} {:>8} {:>8}", "module", "parity", "ported", "missing", "doc-only", "disabled");
    println!("{}", "-".repeat(60));
    // Running totals across every module, so the final row is the whole port at a glance. We sum the raw
    // per-file counts (not the percentages) and re-derive the overall percentage from the summed counts —
    // averaging per-file percentages would mis-weight small files against large ones.
    let (mut tp, mut tm, mut td, mut tx) = (0u32, 0u32, 0u32, 0u32);
    for fp in &parity {
        let t = Tally::of(fp);
        tp += t.ported;
        tm += t.missing;
        td += t.doc_only;
        tx += t.disabled;
        println!(
            "{:<16} {:>6.1}%  {:>6} {:>7} {:>8} {:>8}",
            fp.file,
            t.pct(),
            t.ported,
            t.missing,
            t.doc_only,
            t.disabled
        );
    }
    let total_portable = tp + tm + td;
    let overall = if total_portable == 0 { 100.0 } else { 100.0 * tp as f64 / total_portable as f64 };
    println!("{}", "-".repeat(60));
    println!("{:<16} {:>6.1}%  {:>6} {:>7} {:>8} {:>8}", "TOTAL", overall, tp, tm, td, tx);
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------------------------------------

fn usage() -> ExitCode {
    eprintln!(
        "portcourt — a general C→Rust port evidence court\n\n\
         usage:\n\
         \x20 portcourt check   [config.toml]        verify every module's claim against the evidence (the gate)\n\
         \x20 portcourt explain <fn> [config.toml]   the port status of one function\n\
         \x20 portcourt next    [file] [config.toml] which functions are still missing (the porting driver)\n\
         \x20 portcourt report  [config.toml]        the parity table across all modules\n\n\
         config defaults to ./portcourt.toml"
    );
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    // Hand-rolled arg parsing (no clap dependency): the surface is tiny and the grammar is regular, so a
    // match over the first token plus a couple of positional operands is clearer than a parser framework.
    // The process exit code IS the gate result — `check` returns non-zero on any over-claim — so portcourt
    // drops straight into a Makefile / CI step / git hook with no wrapper.
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Every subcommand resolves its config from the last positional, defaulting to ./portcourt.toml.
    let default_cfg = PathBuf::from("portcourt.toml");
    match args.first().map(String::as_str) {
        // `check [config]` — config is the only (optional) operand.
        Some("check") => {
            let cfg = args.get(1).map(PathBuf::from).unwrap_or(default_cfg);
            cmd_check(&cfg)
        }
        // `explain <fn> [config]` — the function name is required; the config is optional after it.
        Some("explain") => match args.get(1) {
            Some(name) => {
                let cfg = args.get(2).map(PathBuf::from).unwrap_or(default_cfg);
                cmd_explain(name, &cfg)
            }
            None => usage(),
        },
        // `next [file] [config]` — BOTH operands are optional, which is ambiguous when only one is given:
        // is `portcourt next foo.toml` asking to filter to a file literally named "foo.toml", or to use
        // that file as the config? We disambiguate by extension: a single `.toml` operand is the config
        // (the overwhelmingly common intent); a single non-`.toml` operand is a file filter using the
        // default config. Two operands are unambiguous: `<file> <config>`.
        Some("next") => {
            let (file, cfg) = match (args.get(1), args.get(2)) {
                (Some(a), Some(b)) => (Some(a.as_str()), PathBuf::from(b)),
                (Some(a), None) if a.ends_with(".toml") => (None, PathBuf::from(a)),
                (Some(a), None) => (Some(a.as_str()), default_cfg),
                _ => (None, default_cfg),
            };
            cmd_next(file, &cfg)
        }
        // `report [config]` — config is the only (optional) operand.
        Some("report") => {
            let cfg = args.get(1).map(PathBuf::from).unwrap_or(default_cfg);
            cmd_report(&cfg)
        }
        // Anything else (including `--help`, no args, or a typo) prints usage and exits non-zero.
        _ => usage(),
    }
}
