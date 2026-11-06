# Nested recursive ADT local receipts (Scala 3.9.0)

This local evidence increment authenticates exactly four positive roots from
the immutable #2132 `Expr.scala` source (SHA-256
`f700d4e27f11365a08291ae15babc65e673eb91a0bdeee45ea5dc2cf7e8c8607`):
`Evaluator.peelLeft` at `Expr.scala#tree40` / bytes `158:266` and
`Evaluator.sum` at `Expr.scala#tree78` / bytes `270:468`, each in two live,
independently built lexer/parser/namer/typer sessions. The tested baseline is
the merged #2136 commit `0fc457776374f69d6b16187c007721a85b2557a0`.

The positive test constructs both sessions before typing either root. Each row
contains the session nonce and distinct source, store, AST-arena, and fresh
resolver witnesses. Its normalized semantic proof carries the complete root
signature, nested `Add` and `Num` extractor shapes, canonical `Expr`/`Product`
identity, case-local binders, all three distinct recursive `sum` calls and
their `Int` results, and the complete source-span-preserving source-to-typed
mapping count. Arena-local symbol and typed-tree IDs do not enter the
normalized digest. The verifier compares each proof with the checked-in
#2133/#2135 normalized semantic receipts and validates each session witness
independently.

The verifier reuses #2136's normalized evidence interface:
`negative-cases.tsv` supplies the ten frozen Scala classifications, and
`rust-late-failure.tsv` supplies the two late-failure rows. A focused test
executes each pinned source fixture and exact method root, then records the
source ID, root tree and span, owner, and actual typed outcome or diagnostic.
The verifier checks these rows against the pinned source hashes and Scala
classifications. It also requires all three existing bounded-control tests,
the actual `ISSUE2136_NESTED_ROLLBACK` output, and exact tested-revision markers
in the positive, controls, and rollback logs. No failure semantics or source
fixture is added here.

## Local reproduction

Use the pinned Rust toolchain and the same `TMPDIR`, `TMP`, and `TEMP` paths
for every command. The artifacts below are generated under ignored `target/`
and contain no pasted test logs. Run the verifier from a clean committed
checkout; it binds the logs to `git rev-parse HEAD` and requires the pinned
baseline to be an ancestor of that tested commit.

Set `TMPDIR` to the registered scratch directory for the current implementation
workspace, then set `TMP` and `TEMP` to the same path before running these
commands.

```sh
mkdir -p target/typer-nested-adt-audit
cargo test -p dotty-typer --test source_binary_recursive_adt_baseline \
  nested_recursive_adt_original_roots_are_attempted_in_two_fresh_sessions \
  --locked -- --exact --nocapture \
  > target/typer-nested-adt-audit/positive.log 2>&1
cargo test -p dotty-typer --test source_binary_recursive_adt_baseline \
  nested --locked -- --nocapture \
  > target/typer-nested-adt-audit/controls.log 2>&1
cargo test -p dotty-typer --lib \
  nested_late_failures_restore_and_retry_before_original_roots_recover \
  --locked -- --nocapture \
  > target/typer-nested-adt-audit/rollback.log 2>&1
REVISION="$(git rev-parse HEAD)"
python3 tools/typer-classpath-corpus-audit/verify_focused_gate.py \
  --nested-positive-log target/typer-nested-adt-audit/positive.log \
  --nested-controls-log target/typer-nested-adt-audit/controls.log \
  --nested-rollback-log target/typer-nested-adt-audit/rollback.log \
  --nested-output target/typer-nested-adt-audit/positive.tsv \
  --nested-classified-output target/typer-nested-adt-audit/classified.tsv \
  --nested-report target/typer-nested-adt-audit/report.md \
  --nested-tested-sha "$REVISION" --nested-pr-head-sha "$REVISION"
```

The verifier fails closed if a required test did not execute, a log's tested
SHA is missing or differs from the positive log, PR head, or repository HEAD,
the Rust test summary disagrees with observed test outcomes, a pinned case
root/outcome is absent or changed, the two live sessions are not independent,
normalized semantics differ, or the #2136 rollback evidence differs from its
sidecar. It writes the four-row positive TSV, ten-row classified-case TSV, and
compact report with a `PASS` disposition only after all checks succeed. For a
local run before a PR exists, tested revision and PR-head fields use the same
commit; pass the published PR head after publication when verifying that same
commit.
