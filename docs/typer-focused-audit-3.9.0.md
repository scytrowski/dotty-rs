# Focused Typer Audit Modes

The focused audit is a bounded diagnostic for selected Scala 3.9.0 source files
or frozen historical method cohorts. It emits a `FOCUSED / PARTIAL` report and
does not use the full-corpus denominator or full-audit assertions.

## Selections

`typer-focused-smoke-manifest-3.9.0.tsv` is the checked-in smoke manifest. It
selects three source files spanning a small control file, the historical #946
function cohort, and the #1051 backend module/classpath cohort. Manifest paths
are relative to the pinned Scala checkout and are validated against
`library/src` and `compiler/src`. Blank lines and comments are ignored;
duplicate paths are deduplicated and the selected paths are sorted.

Two frozen method cohorts are available:

| Name | Frozen rows | Stable identity | Use |
| --- | ---: | --- | --- |
| `package-term` | #1046 first-blocker rows | source path + method tree index | Recheck package term / module resolution observations |
| `object-module` | #1051 object/module rows | source path + method tree index | Recheck object and ModuleClass completion observations |

The cohort TSV contents are SHA-256 pinned in the selector. Each row is
validated before any Rust test runs; missing source paths, malformed identities,
changed frozen rows, or methods that no longer map to a method tree fail the
selection. Duplicate rows collapse to a unique path/tree identity for execution,
while the frozen TSV remains the source of row-level historical evidence.

## Commands and cost

With the pinned Scala checkout, JDK, and cached Scala artifacts configured as
for `tools/typer-classpath-corpus-audit/run`:

```sh
tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv \
  --verify-determinism --compare-legacy-profile /tmp/typer-focused-smoke.md

tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  /tmp/typer-focused-package-term.md

tools/typer-classpath-corpus-audit/run --focused-cohort object-module \
  /tmp/typer-focused-object-module.md
```

The smoke manifest audits all methods in its three files. A named cohort selects
only its frozen method trees for outcome accounting; the typer runs the nearest
enclosing method body needed to preserve source context, so sibling declarations
in that body may also be traversed. Each outcome row shows both the selected
`method` and the actual enclosing `attempt`. Parsing, AST inventory, and naming
still run for each distinct selected file. `--verify-determinism` executes two
sequential runs (`CARGO_BUILD_JOBS=1`, `RUST_TEST_THREADS=1` are set by the
runner) and compares normalized reports. The test binary is built once before
timing or determinism runs. Runtime goes to the run log as a
separate `FOCUSED_TIMING` diagnostic and is excluded from normalized output.
The report contains only selected-file and selected-method denominators,
first blockers, source/typed outcomes, resolver counters, and deterministic
materialized package/class/object/ModuleClass/member identities. It never
claims a full-corpus count such as `0 / 3297`.

Focused output uses a separate destination and refuses paths matching the full
corpus or import-prefix baseline names. It does not refresh or overwrite their
reports or TSV sidecars. The legacy `--source-file` import-prefix diagnostic
and `DOTTY_AUDIT_PROFILE_FILES` behavior are unchanged.

`--compare-legacy-profile` adds one sequential legacy file-only profile run for
the same selected paths and writes a `-performance.tsv` sidecar. The sidecar
compares wall time, methods attempted/typed, resolver package/member requests,
and peak RSS from `/usr/bin/time` when available. Cohort mode compares its
selected-method run against legacy processing of all methods in the same
files, so method counts describe different scopes; the smoke manifest provides
the direct file-mode comparison. Timing and RSS are measurement data and are
kept outside the deterministic normalized report.
