# Inherited source member baseline (Scala 3.9.0)

This report freezes issue #1903's exact source candidate and records current
whole-`DefDef` behavior. It is an oracle and test baseline; no production Typer,
resolver, or JVM semantics changed. The denominator is the four source-owned
methods in `Child` only.

## Frozen inputs and reproduction

| Input | Value |
| --- | --- |
| `dotty-rs` main | `d455b65981be5f3eafe90ea1e0c4f89156a275e7` |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)` |
| Scala compiler/library | `3.9.0`, source revision `777528f19a58e794c9954a42f433373472ec57f8` |
| JDK | Eclipse Temurin `21.0.8+9` (`JAVA_VERSION=21.0.8`, `IMPLEMENTOR_VERSION=Temurin-21.0.8+9`) |
| Scala artifacts | All eight SHA-256 entries in [`scala-artifacts.sha256.tsv`](../crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/scala-artifacts.sha256.tsv) verified by the runner |
| Frozen source SHA-256 | `f93da74272996204a4c29d07385d3988c811c8084e7fb3352fdb2be00eee26be` |
| Typed snapshot SHA-256 | `8fb288f14037338577785c4b63a0007b6f453a227bac230cefc20d57d6265e12` |

[`evidence.sha256.tsv`](../crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/evidence.sha256.tsv)
pins the checked-in source files, snapshots, negative outputs, baseline TSV,
artifact manifest, and reproduction runner. The runner verifies this manifest
on every oracle run.

The source and normalized Scala snapshot are checked in under
[`inherited-members-3.9.0`](../crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/).
The pinned runner checks the Scala revision, Temurin release, all eight
artifact hashes, the exact `-Vprint:typer` snapshot, and negative diagnostics.
It also compiles both valid shadowing controls. Missing prerequisites or changed
oracle output fail the run.

From the repository root, with the pinned Scala checkout and artifacts
available locally:

```sh
PATH="$JAVA_HOME/bin:$PATH" \
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/maven2 \
crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/run

cargo test -p dotty-typer --test inherited_members_baseline --locked -- --nocapture
```

The Scala snapshot was produced with `-Vprint:typer -Xprint-types
-Yprint-debug -Yprint-debug-owners -color:never`. It shows the canonical
`Parent` declarations (`bias` and `add`), the `Child extends Parent` edge,
`Child` ownership of the four methods, and the compiler's explicit
`this.add`/`this.bias` selections and result types. The lowered lambda bodies
also show `Child.this` for both inherited references. The separate
`ExplicitSelection.scala` snapshot confirms that explicit member forms are
Scala-valid and typed.

## Four-root whole-method result

[`four-root-baseline.tsv`](../crates/dotty-typer/tests/fixtures/inherited-members-3.9.0/four-root-baseline.tsv)
is emitted from parsed tree indices and spans and checked byte-for-byte by the
Rust harness. It records one fully typed root out of four. In particular,
`withBias` succeeds and selects the canonical source-owned `Parent.bias`;
`direct`, `lifted`, and `mapped` remain blocked. There is no 4/4 claim.

Each Rust source session uses one `SemanticStore`, one source index, the real
Lexer → Parser → Namer pipeline, and `SourceTyper::type_method_definition` on
each discovered `Child` `DefDef`. The harness completes the source `Child`
class and independently checks its `ClassInfo.parents` edge and the owners and
kinds of `Parent.add` and `Parent.bias`. A test-only canonical `scala.Function1`
identity is seeded using the same fixture helper as the existing closure
contract tests, so closure roots reach inherited member lookup; no typed tree
is synthesized. Two freshly allocated sessions produce identical normalized
receipts. Successful roots assert the complete `DefDef` and source-to-typed
mapping; blocked roots assert that no root mapping was committed.

| Method | Scala source signature | Scala-valid | Rust whole root | Selected inherited target / first blocker |
| --- | --- | --- | --- | --- |
| `direct` | `(Int): Int` | yes | blocked | `TermNameNotFound(add)` at tree 19, span `148..151` |
| `withBias` | `(Int): Int` | yes | fully typed, result `Int` | `Parent.bias` |
| `lifted` | `(): Int => Int` | yes | blocked | `TermNameNotFound(add)` at tree 35, span `226..229` |
| `mapped` | `(): Int => Int` | yes | blocked | `TermNameNotFound(bias)` at tree 46, span `269..273` |

All three blocked errors are leaf `TermNameNotFound` errors, so their nested
first-error chain is one item; the TSV keeps the exact missing name, source
tree, and span. In the two closure roots the failure occurs in the lambda body
before closure capture is reached. These rows therefore establish an inherited
lexical lookup blocker inside closures, not a separate capture failure.

## Separate diagnostic controls

The Rust control source keeps these checks outside the fixed four-method
denominator. Unqualified `direct` fails while `this.add(x)` completes and
selects the exact source `Parent.add` symbol. Both the implicit and explicit
field controls complete and select `Parent.bias`. Scala's normalized snapshot
agrees that the explicit forms have the same result types as their implicit
counterparts.

Scala 3.9.0 also compiled the positive shadowing controls: a child override
shadows an inherited same-name method, a local `bias` shadows an inherited
field, and two unrelated classes with an `add` member resolve through their
own receiver types. The negative controls were rejected:

| Control | Scala 3.9 classification | Observed diagnostic |
| --- | --- | --- |
| missing member | invalid | `Not found: absent` |
| wrong argument `add(true)` | invalid | found `Boolean`, required `Int` |
| inherited private method and field | invalid access | both names are not found in `Child` |
| shadowed member and local | valid | compiled successfully |
| unrelated same-spelled `add` methods | valid | compiled successfully |

The expected negative output is checked in separately for each invalid source.
No Scala-valid-but-deferred Rust case is classified as invalid.

## Current component boundary

The source `Parent` → `Child` hierarchy and canonical declaration ownership are
present and verified. Inherited explicit selection works for the method and
field controls, and the four-root `withBias` success proves ordinary inherited
field lookup already works. The uncovered behavior in this cohort is
unqualified inherited method lookup, including the same lookup from a lambda.
Unqualified inherited field lookup inside `mapped` also fails, despite the
non-closure `withBias` root succeeding. This points to lexical lookup being
context-sensitive across body shapes; it does not establish a general member
adaptation or class-hierarchy defect.

Existing same-owner closure coverage establishes that `FunctionLiteralIndex`
can type closures that capture the enclosing `this`. Since the inherited
closure roots stop at their missing member identifiers, this baseline does
not claim that inherited `Child.this` capture itself fails or succeeds in the
Rust typer. Generic parents, parent traits, multiple inheritance, `protected`,
`super`, and JVM/TASTy behavior remain deferred.
