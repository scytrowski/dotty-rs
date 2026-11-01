# Heterogeneous Product2 source Match methods (Scala 3.9.0)

Issue #1970 adds a focused integration proof for the immutable Product2 source
fixture at
`crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/Positive.scala`.
Its SHA-256 remains
`4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31`, matching
the original #1958 baseline. The pinned Scala roots remain
`Positive.scala#tree24` (`Matcher.select`, bytes 86:208) and
`Positive.scala#tree44` (`Matcher.classify`, bytes 212:332). The case class is
`Positive.scala#tree8`; both methods belong to the source `Matcher$` module
class at tree 48.

## Result

Both original method trees type as complete owned `DefDef`s through the real
Lexer, Parser, Namer, and `SourceTyper::type_method_definition` path. The test
uses the authentic Scala library Product TASTy/JDK classpath resolver and keeps
one `SourceTyper` alive while typing both roots, so both methods observe the
same canonical external `scala.Product` identity. Two independent source
sessions produce identical normalized semantic receipts.

`select` maps its source Match, CaseDef, UnApply, extractor qualifier, nested
patterns, binders, If, branch expressions, result type, and original method
tree into the typed index. Its original companion-owned generated
`Flagged.unapply` returns `Flagged`; source-owned selectors `_1: Int` and
`_2: Boolean` retain their constructor-field provenance. The case creates
independent `value: Int` and `enabled: Boolean` binders. The condition resolves
to the enabled binder, the then branch resolves to value, and the branches
join to `Int`.

`classify` keeps its two source-ordered cases. Its first extractor checks the
constant `0: Int` against `_1` and `true: Boolean` against `_2`. Its second
extractor creates one `value: Int` binder and preserves `_` as a Boolean
wildcard. The final Match type is `Int`; the first case has no binder.

The compact exactly-two-roots receipt is
[`rust-after-source-match.tsv`](../crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/rust-after-source-match.tsv).
The earlier #1958 first-blocker and #1968 selector receipts remain unchanged.

## Failure and rollback controls

Focused Rust regressions reject wrong literal/component types, extractor
arities one and three, a declared result mismatch, a cross-case binder escape,
a non-case class without the original generated extractor identity, and a
resolver-free `scala.Product` identity. Each failure leaves the typed arena,
semantic store, and source-to-typed index unchanged. A late Boolean result
failure after pattern binders are created rolls back before the valid sibling
`classify` method is typed in the same session.

The pinned Scala 3.9.0 Product2 runner was executed for this change with source
revision `777528f19a58e794c9954a42f433373472ec57f8`, all eight SHA-256-verified
Scala 3.9.0 artifacts, and Temurin 21.0.8+9. Its frozen typed snapshot, source
manifest, output hashes, and rejection matrix matched. The matrix classifies
the wrong literal types, arities one and three, wrong result, and ordinary
non-case extractor controls as rejected. The existing Box extractor oracle
classifies case-binder escape as Scala-invalid. Scala-valid but Rust-deferred
pattern cases remain labeled as deferred in the preexisting controls.

Run the focused Rust proof with:

```sh
cargo test -p dotty-typer --test source_case_class_match --locked
```

Re-run the pinned Scala oracle using the existing fixture runner and inputs:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/temurin-21.0.8+9 \
COURSIER_MAVEN_ROOT="$PWD/target/coursier/v1/https/repo1.maven.org/maven2" \
DOTTY_PRODUCT2_TMP_ROOT="$PWD/target/source-case-class-product2-oracle" \
crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/run
```

No production typer, namer, backend, IR, or TASTy behavior changed. The change
adds source-level regression coverage and this receipt only.
