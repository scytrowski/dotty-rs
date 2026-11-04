# Binary recursive `eval` typer proof (Scala 3.9.0)

Issue #2078 adds a focused source-to-typed proof for the original
`Evaluator.eval(expr: Expr): Int` in
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/Expr.scala).
It uses the frozen source and authentic Product/JDK resolver in two independent
sessions. The first types `eval` before `selectLeft`; the second reverses that
root-attempt order. Both sessions normalize to the same receipt, and retyping
`eval` in each session reuses its published typed root.

The Add case retains the two source pattern positions and their source-derived
field selectors: `left` is a distinct case-local `Expr` binder for `Add._1`,
and `right` is a distinct case-local `Expr` binder for `Add._2`. The typed case
contains two distinct recursive `Apply` trees. `eval(left)` and `eval(right)`
each pass the corresponding original binder reference and both select the
canonical source `Evaluator.eval` method. Each call has result `Int`; the
left-to-right `Int + Int` application and the complete Match also have result
`Int`. The original `Num(value)` case remains mapped and returns its `Int`
binder. All 25 nonempty source trees within the method preserve their spans in
the typed tree.

The exact before/after observation is pinned in
[`rust-eval-two-call-before-after.tsv`](../crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/rust-eval-two-call-before-after.tsv).
The test compares normalized receipts across the two independent sessions.
Existing selectLeft provenance and rollback controls continue to run alongside
this proof. The comprehensive negative and transaction matrix remains in
#2079.

No production typer behavior changed: the frozen method already completed on
the integrated base. This increment adds assertions and evidence for the two
independent recursive calls and their argument identities.

Run the focused proof with the pinned Rust toolchain and a Java 21 runtime:

```sh
cargo +1.98.1 test -p dotty-typer --test source_binary_recursive_adt_baseline \
  binary_recursive_eval_preserves_two_independent_add_binders_and_calls \
  --locked -- --nocapture
```
