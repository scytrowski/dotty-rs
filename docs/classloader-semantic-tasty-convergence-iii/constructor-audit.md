# Semantic TASTy convergence III: authentic ordinary constructor audit

Status: audit complete; this report defines the bounded B2 constructor gate. No
production code changed.

The loader rejection below records the #2026 audit snapshot before issue
[#2028](constructor-one-int.md); the latter implements the bounded B2 form
from that audit without changing this historical fixture ledger or its TSV.

## Frozen inputs and provenance

The original compiler-generated inputs are the unchanged
`crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorPlain.tasty` and
`CtorWithVal.tasty`, with source `Constructors.scala`. They were compiled with
Scala 3.9.0 from revision
`777528f19a58e794c9954a42f433373472ec57f8`; TASTy header is 28.9. The
reproducible test pins all three SHA-256 values and verifies that the source
contains `class CtorPlain(x: Int)` and `class CtorWithVal(val x: Int)`. The
exact digests and the two audited rows are in
[constructor-audit.tsv](constructor-audit.tsv).

The audit ran on fresh main `1ab217d716f47d68ba46915cfdea2fb842fcb173`, Rust
`1.98.1`, and Temurin JDK `21.0.13+9` (JDK 21 `jmods`). The resolver classpath
places an ephemeral package-relative copy of the original TASTy bytes before
the repository's fixed Scala/JDK fixtures; the fixture hashes are checked
before copying. No producer bytes are regenerated. Historical reports and
cohort baselines are unchanged.

## Source and AST evidence

`CtorPlain` is the positive shape: the TASTy package is
`me.cytrowski.tastyfixtures.semantic`, the root class is `TYPEDEF@4`, and its
primary constructor is `DEFDEF@30`. The semantic probe enters and explicitly
completes both original addresses in a fresh `SemanticStore` with one
`Definitions::bootstrap` and the real classpath resolver. The result is
repeatable across two independent stores. The constructor symbol is owned by
the entered class, has one plain `Method` clause with one formal whose type is
the canonical bootstrapped `scala.Int`, and its effective result points to
that exact class symbol. The original constructor return tree is a `NatAst` whose payload resolves
through the TASTy name table to `Unit`; the test records that serialized
`Unit` separately from the effective result, which resolves to the exact
entered class. Constructor completion applies the existing
constructor-result normalization.

`CtorWithVal` remains a neighboring negative control. Its original class and
constructor addresses are `TYPEDEF@4` and `DEFDEF@28`. Its source is explicitly
`val x: Int`; the fixture's header field at address 9 and constructor formal at
address 31 enter as distinct symbols. The ordinary formal shape alone does not
make this an eligible class: the header field/alias is additional member
semantics outside B2. The fixture is never used as the positive acceptance
input.

## Loader boundary at the audit snapshot

Calling the existing crate-private semantic loader on the unchanged
`CtorPlain.tasty` returns, before the semantic transaction begins:

```text
UnsupportedShape("the class template must have no parameters and one constructor declaration")
```

The gate currently rejects the template's ordinary term parameter together
with other class-header parameters. Its check runs before symbol entry or
transactional completion, so `semantic_tasty_loaded` remains empty. This is
the observed first rejection; later constructor-specific restrictions are not
claimed as the current first failure.

The legacy `load_class` path was run independently in a separate store against
the same fixture and real JDK 21 classpath. It succeeds and returns a class
symbol. This is a separate legacy decoding result, not evidence that the
semantic loader accepts the class.

## Negative controls

The existing loader structural regressions cover a mismatched requested
package/name, malformed/truncated TASTy, and unsupported constructor/template
shapes with typed failures before mutation. The real JDK was available for the
positive resolver-backed completion; the legacy run also resolved
`java.lang.Object`. A missing-JDK run is an environment/dependency negative,
not a Scala constructor rejection, and is deliberately not used to classify the
semantic gate.

## Minimal B2 contract

B2 may admit only the exact bounded constructor form established here:

- one top-level class with no class type parameters, companions, annotations,
  or extra declarations;
- exactly one ordinary constructor parameter clause, containing exactly one
  formal;
- that formal resolves to the canonical `scala.Int` symbol from the same
  bootstrapped store and is owned by the constructor;
- no `val`/`var` header field or generated accessor; contextual, implicit,
  erased, polymorphic, curried, secondary, and additional-parameter forms stay
  unsupported;
- constructor ownership is the entered root class, and its completed method
  result references that exact class identity (serialized `Unit` is not copied
  as the effective result);
- retain the current bounded transaction/checkpoint protocol: classpath
  resolution and unpickling happen under one checkpoint, package/scope/origin
  publication occurs only on success, and every typed failure rolls back
  symbols, types, scopes, caches, aliases, and root publication.

The two input cases required no new resolver, field/accessor, or transaction
capability. No adjacent constructor form is inferred as supported.
