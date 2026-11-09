# Original `Option.map` two-session acceptance (#2182)

This receipt covers the unchanged Scala 3.9.0 source root
`library/src/scala/Option.scala#tree=214` (`scala.Option.map[B]`). The source
checkout is pinned to `777528f19a58e794c9954a42f433373472ec57f8`; the complete
`Option.scala` file SHA-256 is
`fadeb6620ab6cdadfd539409e335b38142fa1f6ba6b9143b2d6cfcb90e0076c3`.
The pinned runtime/compiler artifact hashes are recorded in
`crates/dotty-typer/tests/fixtures/lexical-closure-contract/scala-artifacts.sha256.tsv`.

## Executable proof

The focused regression is
`captures_acceptance_iv_relation_operands_before_transaction_rollback` in
`crates/dotty-typer/src/typer/tests/issue_2179_relation_observation.rs`.
It reparses and names the original source and uses the ordinary
`SourceTyper::type_method_definition` path. Before the successful retry, a
one-shot post-success failure checks that the semantic store, typed arena,
type cache, typed indexes and source-to-typed index all return to their prior
checkpoints, including no publication for the method root.

The retry then checks the canonical `.scala.Option.map` owner and source
definition, the method-owned `B` binder, the preserved polymorphic method
signature, and its exact `Option[B]` result (`ParamRef` index 0 under the
method binder). The complete typed root remains a `DefDef` with the original
conditional RHS. Its branches retain canonical `scala.None` and
`scala.Some[B]`, the latter selecting canonical `scala.Some$.apply`; the
conditional retains the `None | Some[B]` join. Every source-to-typed mapping
preserves its source identity and remains within the original source span.
The asserted mapping is:

```text
195:0,196:1,199:2,200:3,203:4,204:5,205:5,206:6,207:7,208:9,209:11,210:12,211:13,212:14,213:15,214:16
```

The same test retains the unrelated original `Tuple.toArray` negative control;
its unsupported receiver conformance remains an explicit separate outcome.

On the implementation checkout, two separate local Cargo test processes
passed and emitted byte-identical normalized acceptance lines
(SHA-256 `87495844124e0ce0162ef36296a891eb89985facc121bafe0c7e90d04e9642ac`).

## Independent sessions and protected cohorts

`.github/workflows/typer-focused-audit.yml` runs this ignored external-input
test in two separate Cargo test processes. It requires both full test runs to
pass, extracts their normalized acceptance lines, compares them byte-for-byte,
and uploads both logs and a receipt with source and tested revision hashes.
Each run creates its own parser, semantic store, resolver, source index,
typed arena and `SourceTyper`; cross-session numeric arena IDs are not used for
comparison.

The same required workflow also runs the frozen 25-root main and 25-root
analysis ordinary-method audits twice each, with the existing exact protected
root contracts and deterministic report comparisons. It does not modify their
manifests or reports. The workflow artifact is the hosted evidence record;
this repository note describes the gate and does not claim a hosted run before
one completes for the reviewed PR head.

Local deterministic cohort runs on the implementation checkout retained the
existing results: main `18/25` (SHA-256
`304f67ba1454383bd4dbc3a7e5c9d4d30e6c6e4cf15a41295d05eed2335a6bf5`) and
analysis `16/25` (SHA-256
`671b3b9e1037d0416b228ab634b2cde4515beecab0926ae945488eb95eb9ed01`). Both
existing required-root gates passed in deterministic two-pass mode.
