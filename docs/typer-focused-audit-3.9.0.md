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
  --verify-determinism --compare-legacy-profile target/typer-audits/typer-focused-smoke.md

tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  target/typer-audits/typer-focused-package-term.md

tools/typer-classpath-corpus-audit/run --focused-cohort object-module \
  target/typer-audits/typer-focused-object-module.md
```

The runner stores scratch files under
`target/typer-classpath-corpus-audit-tmp` by default, avoiding the shared system
`/tmp`. Set `DOTTY_AUDIT_TMP_ROOT` to a task-specific workspace directory when
the per-run logs and intermediates must be retained for comparison or review.

The smoke manifest audits all methods in its three files. A named cohort selects
only its frozen method trees for outcome accounting; the typer runs the nearest
enclosing method body needed to preserve source context, so sibling declarations
in that body may also be traversed. Each outcome row shows both the selected
`method` and the actual enclosing `attempt`. Focused reports also include each
resolver request's target, outcome, cause, materialized prefix, and selected
identity. The frozen cohort sidecar joins those observations to each historical
row by enclosing attempt and target, preserving explicit missing-attempt and
missing-target states instead of copying a method-level blocker onto every row.
Parsing, AST inventory, and naming
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

### Initial smoke measurement

The checked-in [#1281 smoke measurement](typer-focused-audit-1281-smoke-performance.tsv)
uses the three manifest files, Scala revision `777528f19a58e794c9954a42f433373472ec57f8`,
Scala 3.9.0 artifacts, and JDK 21.0.8. The two focused runs took 3.210 s and
3.222 s; the legacy file profile took 3.981 s. All three attempted the same 31
methods, typed zero, and made 269 package plus 102 member resolver requests.
This is one local smoke sample, not evidence of a general 20% speedup: the
legacy mode emits per-file and per-method timing diagnostics that focused mode
does not. Peak RSS is `unavailable` because GNU `/usr/bin/time` is not installed
on this worker. The focused normalized reports matched byte-for-byte.
