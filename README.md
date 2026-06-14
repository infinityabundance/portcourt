# portcourt

A general **C→Rust port evidence court**. A faithful port lives or dies by one question: *is the claim
backed by the evidence?* portcourt makes that mechanical.

It consumes the parity view a port already produces — the typed join of every C source symbol against the
real Rust `fn` that ports it (`active`/`missing`/`doc_only`/`disabled`) — plus a small declaration of what
each module *claims*, and enforces **closure math**:

> A module that claims to be `complete` must have **zero missing functions** and **zero doc-only false hits
> on compiled functions**, and every court it `requires` must actually be **sealed** — otherwise the build
> fails. You cannot claim more than the evidence proves.

## Subcommands

| command | what it does |
|---|---|
| `portcourt check [config.toml]`        | the gate — verify every module's claim against the evidence (nonzero exit on over-claim) |
| `portcourt explain <fn> [config.toml]` | the port status of one function: ported? missing? a doc-only false hit? a `#if 0` non-target? |
| `portcourt next [file] [config.toml]`  | the porting driver — which functions are still missing (and which doc-only comments to reword) |
| `portcourt report [config.toml]`       | the parity table across all modules |

## Config (`portcourt.toml`)

```toml
parity = "reports/port-index/parity-detailed.json"   # the C↔Rust symbol join (per file, per fn)
courts = "reports/claim-ladder.json"                  # optional: the sealed courts, for `requires`

[module."fileio.c"]
claim = "complete"                                    # complete | partial
requires = ["GNURUST.FILEIO.INDEXED.1", "..."]        # courts that must be sealed for this claim

[module."common.c"]
claim = "partial"
min_parity = 0.0                                      # optional floor on active parity %
```

The parity JSON is any array of `{ "file": "...", "fns": [{ "function", "rust_status", "preprocessor_status" }] }`.
portcourt is deliberately port-agnostic: point it at your parity view and claim ladder and it works.

## Why "closure math"

The name is the discipline: a claim is a *closure* over its evidence. `check` computes whether the closure
holds — every required court sealed, every compiled function ported, no doc-only inflation — and refuses to
let a port assert completeness it hasn't earned. It is the same anti-over-claim rule a careful port already
applies by hand, made into a gate.

## License

Apache-2.0. No dependency on, or derivation from, any ported source — portcourt only reads evidence.
