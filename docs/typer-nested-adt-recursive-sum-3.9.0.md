# Nested recursive `Evaluator.sum` source-Typer proof (Scala 3.9.0)

This proof covers the unchanged `Evaluator.sum(expr: Expr): Int` root in the
immutable fixture
[`Expr.scala`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/Expr.scala),
SHA-256 `f700d4e27f11365a08291ae15babc65e673eb91a0bdeee45ea5dc2cf7e8c8607`.
The acceptance root is the original three-case method; it is not replaced by
the foreign-extractor probe or another helper. The Scala reference is pinned
to revision `777528f19a58e794c9954a42f433373472ec57f8`.

The real Lexer → Parser → Namer → `SourceTyper::type_method_definition` path
completes the whole source-owned `DefDef` in two independent
resolver/store/arena sessions on integrated main `73748e6`. Both sessions
preserve the original ordered `Num(value)`, `Add(Num(value), right)`, and
`Add(left, right)` cases, canonical source-owned `Evaluator.sum`, and all
source-to-typed mappings with original spans. The nested `Num._1` selector is
`Int`; the two `Add` selectors are distinct `Expr` fields. Five case binders
have independent identities and the expected `Int` / `Expr` types. The three
recursive call trees use the right, left, and second right binders in source
order, all resolve to the same canonical method, and all return `Int`. Both
`Int + Int` trees, each branch, the complete match, and the method have an
`Int` result. Same-session retyping reuses the same typed root.

The typed graph preserves the enclosing edges as well: the `DefDef` RHS points
to its mapped `Match`, each ordered typed `CaseDef` points to the mapped
pattern and block, and each typed block expression points to the mapped source
result. For both additions, the typed selected `+` receiver and argument are
exactly the mapped source left and right operands. This ties the recursive
calls in the receipt to the operands that actually feed the method result.

The normalized receipt is
[`rust-sum-complete.tsv`](../crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0/rust-sum-complete.tsv).
The focused regression compares it across fresh sessions and pins original
spans without depending on allocated arena IDs. A separate positive control
declares `sum` before `peelLeft` and confirms the original sum root still
types, including its three canonical recursive calls.

Bounded invalid-`sum` controls require failure without partial typed trees or
source mappings. The scope control uses a malformed sibling copy of the same
three-case body, leaving the immutable acceptance root available for its
same-session retry:

| Control | Rust typer diagnostic |
| --- | --- |
| Invalid recursive argument | `ApplicationArgumentTypeMismatch` |
| Recursive call arity mismatch | `ApplicationArityMismatch` |
| Sibling case binder escapes into the third `sum` case | `TermNameNotFound` |
| Wrong nested `Num.unapply` owner | `SymbolSourceKindMismatch` |
| Incompatible method result | `OverloadApplicationNoApplicable` |

The successful root proof also checks pairwise distinct binder symbols,
including the two separately spelled `right` binders, so an alias regression
fails directly. The escaped-binder control types a malformed sibling method
through its first recursive call, checks full rollback, then successfully
types the unchanged original `sum` in the same session. The corrupted
nested-owner control restores the canonical owner and also retries in the same
session. No production Typer, Namer, or core change was needed; the prior
baseline already completed this root, while this increment makes the coupled
identity and failure evidence explicit.

Reproduce the focused evidence with:

```sh
cargo +1.98.1 test -p dotty-typer \
  --test source_binary_recursive_adt_baseline \
  --locked -- --nocapture
```
