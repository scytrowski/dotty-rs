# Local pattern definition contract: Scala 3.9.0

The oracle sources live in
[`tools/scala-parser-oracle/fixtures/compilation/patdef-lowering-3.9.0.scala`](../tools/scala-parser-oracle/fixtures/compilation/patdef-lowering-3.9.0.scala)
and
[`tools/scala-parser-oracle/fixtures/compilation/patdef-binder-visibility-3.9.0.scala`](../tools/scala-parser-oracle/fixtures/compilation/patdef-binder-visibility-3.9.0.scala).
They are pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The parser oracle compares their
source `PatDef` shapes. The lowering contract below is normalized from that
revision's `compiler/src/dotty/tools/dotc/ast/Desugar.scala`, especially
`patDef`, `makeSelector`, and `makePatDef`.

## Normalized lowering

Dotty handles each source pattern in a `PatDef` independently, applying the
PatDef-wide type tree to every pattern first. A simple identifier is emitted
as one final `ValDef` carrying the RHS directly. For a general pattern, Dotty
collects non-wildcard variable binders and lowers the check/extraction as
follows:

Variables inside a pattern `Alternative` are diagnosed as illegal by the
pinned `Desugar.getVariables` implementation and are not returned as final
PatDef binders. The corpus audit therefore retains `Alternative` as the root
shape but stops binder collection at that node.

| User binders | Normalized result |
| --- | --- |
| One | One final `ValDef`; its RHS performs the pattern match/check and yields the binder value. |
| Zero, strict definition | Keep the lowered RHS/check as an expression. No user binder is emitted; later phases may materialize the expression in a synthetic temporary. |
| Zero, lazy definition | Keep a lazy synthetic result so the RHS/check remains deferred until demanded. |
| Multiple | Evaluate the lowered RHS/check once into a synthetic aggregate, then emit one final definition per binder that selects its component. Lazy source modifiers make the aggregate lazy and lower each exposed binder through a `DefDef`. |

For multiple binders, the aggregate is a conceptual contract; generated names
and some simple tuple optimizations are compiler-private. When the RHS is a
syntactic tuple aligned with a simple tuple pattern, Dotty may project the
tuple directly without a temporary. The general pattern path emits a single
`Match` around the RHS and returns the binder tuple. A simple matchable tuple
may instead bind the whole tuple once and project its elements.

The pinned `-Vprint:typer` oracle was compiled with the Scala 3.9.0 artifacts.
After normalizing generated temporary names, its representative shapes are:

```text
one binder:       val x = rhs @RuntimeChecked match { case pattern => x }
zero binders:     retain RHS/check expression; emit no source binder
multiple binders: val $tmp = rhs @RuntimeChecked match { case pattern => (a, b) }
                  val a = $tmp._1
                  val b = $tmp._2
lazy binders:     lazy val $tmp = match; def a = $tmp._1; def b = $tmp._2
unchecked tuple:  keep a Match; do not replace it with tuple projection
```

`tupleBinders` uses a call returning a tuple, so the oracle emits one match,
one synthetic aggregate, and two projections. `zeroBinders` retains the
selector check; the typed-tree printer represents the expression statement
with an unused synthetic value. `uncheckedTuple` confirms that an annotated
selector does not take the direct tuple-projection path.

PatDef selector checks use Dotty's `IrrefutablePatDef` match-check attachment.
During typing, the checker determines whether the pattern is irrefutable; for
a refutable check it gives the selector an inferred `@RuntimeChecked` type and
reports the Scala 3.9 migration warning. An explicit `@unchecked` or
`.runtimeChecked` selector already carries the unchecked marker. Tuple
optimizations are not used when the RHS may carry either marker, because that
can change runtime type-test behavior. The `uncheckedTuple` and
`runtimeCheckedTuple` fixtures pin both boundaries; this issue does not define
warning policy or implement irrefutability analysis.

The fixture's tuple destructuring has a known `(Int, String)` RHS and produces
no refutability warning. Since the RHS is a method call rather than a tuple
literal, Dotty still preserves the selector match and projects the resulting
aggregate. `Some(x)` against `Option[String]` is treated as a refutable
extractor check and emits the pinned migration warning; accepting
`.runtimeChecked` or `@unchecked` still permits a `MatchError` at runtime.

## Visibility

The positive fixture uses a pattern binder in the immediately following
statement. Its final binder definitions are emitted at the PatDef's source
position and are visible after the definition. The negative fixture refers to
`later` in the pattern-definition RHS before the following local value is
initialized; Scala reports a forward-reference error. The lowering uses the
source RHS for the generated check, so the binder's availability does not make
later local values initialized earlier.

## Typed block expansion convention

Block typing associates each source statement with one typed `anchor` in the
`SourceTypedIndex`, while the enclosing typed block may contain an ordered
list of emitted statements for that source statement. Ordinary statements use
the same tree as both anchor and sole emitted statement. A lowered PatDef may
emit several typed statements, but its source PatDef still maps only to its
single anchor. User binder identities remain available through the existing
`local_symbol_at` mapping on the source binder tree; compiler-generated
temporary symbols are recorded as PatDef expansion metadata and do not receive
source-tree mappings. Expansion metadata, semantic symbols, and typed mappings
participate in the enclosing expression transaction.

The current typer increment implements one source pattern with exactly one
user-visible binder when it is an immutable inferred local `val`. It types the
RHS once, then reuses that typed tree as the selector of the synthetic
`Match`; the `Match` result becomes the final `ValDef` RHS so refutable patterns
retain their runtime failure behavior. Pattern typing runs in a temporary case
scope. The typed source binder node remains mapped to the temporary typed
`Bind`, while `local_symbol_at(source, binder_tree)` records the distinct final
block-local symbol. The PatDef source root maps to the final `ValDef` anchor.
The final symbol enters the block scope only after RHS and pattern typing have
succeeded. Modifier-bearing, explicitly typed, missing-RHS, zero-binder, and
multi-binder forms remain deferred.

## Scope

The fixtures cover extractor and tuple roots, zero/one/multiple binders,
wildcards, aliases, `var`, `lazy val`, a PatDef-wide annotation, `@unchecked`
and `.runtimeChecked` RHSes, and binder visibility. They pin the inputs and
normalized lowering contract; the typer supports the bounded one-binder
inferred-`val` slice described above.
