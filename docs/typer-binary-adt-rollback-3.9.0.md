# Binary recursive ADT rollback audit (Scala 3.9.0)

This audit closes the proof gap left by #2078. It keeps the original `Expr.scala`
positive methods unchanged and adds test-only failure controls, a classified
source matrix, and direct structural corruption rows. No production typer
semantics changed.

## Frozen inputs

The audit started from integrated main `e309e1f950ffe9dc0cda25647086f64f6c9b7b60`
(after #2078 merge `5b9e24ab3f16bc8f500d99f99b3277079d4b00ff`). The #2112
descendant changes command-timing evidence only. The immutable positive
`Expr.scala` is SHA-256
`1d788b3bd570df83ce2a053c29f4e48145a93815c7dea5880c71d46573dd0b0a`; the
pinned Scala typed snapshot is
`a95a9cc3c9c71b1af87a305f810404f6b1bc5f95c8914e16d0e1ad49eaea1298`.

The runner verified Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin `21.0.8+9`, and
all eight artifact digests in
[`scala-artifacts.sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/scala-artifacts.sha256.tsv).
The final source and compiler-output manifest hashes are recorded in
[`source-hashes.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/source-hashes.tsv)
and
[`output-sha256.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/output-sha256.tsv).

## Rollback and recovery proof

`binary_late_failure_restores_complete_state_retries_then_types_both_original_roots`
first types an unrelated `prior` root. A private failure observer runs after
both original Add binders have complete `Expr` types and both ordinary
`Evaluator.eval` Apply trees map to their corresponding binder references and
the canonical method symbol. It also sees the typed Match before the late
Boolean-result mismatch unwinds.

After the failure and its repeated retry, the test compares the full existing
#1999 projection plus source-type cache contents, generated-entry journals,
import caches, local-method indexes, typer frames, resolver checkpoint, typed
nodes, semantic types and scope buckets. The successful `prior` mapping remains;
the failing DefDef, Match, calls, arguments and binders leave no source-typed or
pattern-binding mappings. The original `selectLeft` and `eval` roots then type
for the first time in the same live `SourceTyper`. Their normalized semantic
receipts match a second independently parsed and resolved source session.

The test found no leaked state and required no production fix. Existing test
controls still cover a failure in the second recursive argument after the
first call has typed. The frozen source controls separately classify first and
second bad arguments and recursive call arity.

## Classified controls

The pinned Scala runner and Rust test harness cover 22 source controls. Scala
classifies 13 as rejected, 7 as accepted, and 2 as accepted with a warning.
Rust records 6 accepted roots and 16 deferred diagnostics. Each Rust row stores
the selected root and span, the first diagnostic tree and span (or normalized
accepted owner/type/call receipt), same-session retry, and fresh-session parity
in [`rust-control-matrix.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-control-matrix.tsv).
The rows include the three Add extractor arities, nested literal/reference
types, both recursive argument positions, zero/two argument call arity, both
result failures, scope escape, wrong/missing parent, foreign same-spelled
extractors and evals, generic/applied reference fields, unrelated reference
fields, explicit unapply/selector collisions, and warning-only matches.

Rust-only semantic-store corruption is tested separately because those inputs
are not Scala source. Eight named rows cover swapped and aliased selector scope
identities, swapped source provenance, foreign selector/unapply owners and
companion links, wrong parent and broken Product identity. Each row checks
complete symbol/type/scope/annotation contents, source indexes, scope buckets,
the exact error, and deterministic retry in
[`rust-structural-controls.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-structural-controls.tsv).

The acceptance-to-evidence index is
[`typer-binary-adt-rollback-3.9.0-coverage.tsv`](typer-binary-adt-rollback-3.9.0-coverage.tsv).

## Validation

The fixture runner was executed with the pinned Scala checkout, Temurin JDK,
artifact cache, and scratch directory under the issue worktree. Focused Rust
validation uses:

```sh
cargo test -p dotty-typer recursive_adt_rollback --locked
cargo test -p dotty-typer --test source_binary_recursive_adt_baseline --locked
```

The workspace formatter and frontend dependency-boundary guard are also part of
the required checks for this increment. No hosted corpus workflow or CI wiring
was added.
