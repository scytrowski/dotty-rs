# Binary recursive ADT E2E gate (Scala 3.9.0)

This gate binds the frozen `Expr.scala` source from #2061 to one mandatory,
run-bound source-Typer proof. Its only positive roots are the original
`Evaluator.selectLeft` and `Evaluator.eval` DefDefs, each completed in two
separately constructed sessions. The hosted artifact
`binary-recursive-adt-e2e-report.md` records the evaluated PR head, integrated
baseline, source and toolchain pins, workflow run, and disposition.

The positive sidecar has exactly four rows. Each row records the source tree and
span, normalized semantic digest, and source owner, complete signature, full
Match, Product2 identity graph, field and selector provenance, and complete
source mapping proof. The verifier compares semantic rows across sessions
without comparing arena-local numeric IDs. A missing, duplicate, substituted,
partial, stale, or malformed row fails the gate.

The negative/rollback sidecar combines the pinned Scala 3.9 classifications,
the 22 source control outcomes, eight structural identity corruptions, and the
real failure after both Add binders and both recursive Apply nodes exist. It
records rollback, repeated failure, same-session recovery, and normalized
fresh-session parity. The protected-cohort sidecar records all 25 main and 25
analysis roots across two deterministic passes, including the unchanged 16 and
15 protected root sets.

All three TSVs are produced by the existing `typer-focused-audit.yml` run and
uploaded with its report artifact. Their rows carry the actual baseline,
tested-source, and PR-head SHAs; checked-in fixture manifests remain immutable.
The Scala oracle is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`, Eclipse Temurin JDK `21.0.8+9`, and
the eight artifact digests in the binary fixture's `scala-artifacts.sha256.tsv`.
The focused workflow also authenticates merge ancestry for issues #2061,
#2066, #2071, #2072, #2076, #2078, and #2079.

Run the focused acceptance locally from the repository root with Scala 3.9.0,
Temurin 21.0.8+9, and the pinned Rust toolchain installed:

```sh
cargo +1.98.1 test -p dotty-typer --test source_binary_recursive_adt_baseline \
  select_left_completes_first_with_authentic_two_field_provenance_in_fresh_sessions \
  --locked -- --exact --nocapture
cargo +1.98.1 test -p dotty-typer --test source_binary_recursive_adt_baseline \
  binary_recursive_eval_preserves_two_independent_add_binders_and_calls \
  --locked -- --exact --nocapture
cargo +1.98.1 test -p dotty-typer --lib --locked recursive_adt -- --nocapture
```

The workflow also runs the pinned Scala positive and negative oracle, structural
corruption controls, two-pass protected cohorts, existing 18-root suite, and
required workspace/JDK checks. The hosted full Scala corpus remains disabled.
The report says `SPRINT GOAL UNMET` until the mandatory focused run and an
independent review of the same PR head have both succeeded.
