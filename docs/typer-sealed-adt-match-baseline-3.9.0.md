# Sealed `Event` match baseline (Scala 3.9.0)

This report freezes issue #1992's denominator and first-blocker evidence. It is
test and fixture infrastructure only; it records current typer behavior and
does not change Namer or Typer semantics.

## Frozen oracle

The complete source under test is
[`Event.scala`](../crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/Event.scala)
(SHA-256 `a3554b9a764b612af5b6b73779839af82a5e448e909464e45cbc18fb8a86cba1`).
The oracle is Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8`, with the eight compiler/runtime
artifacts pinned by `scala-artifacts.sha256.tsv`. The reproducible runner checks
that revision, the artifact digests, all frozen Scala input digests, and the
Temurin `21.0.8+9` release before comparing compiler output. The focused audit
workflow supplies that JDK and archives its environment record and oracle log.
The typed tree snapshot is
`Event.scala39-typed-tree.txt` (SHA-256
`54c4e0754d9dde79b5b1198ae390db679621623c7e146bfe5d91abd3e04461fa`).

The denominator is exactly the two original `DefDef` roots:

| Root | Owner | Source span | Pinned Scala 3.9 result |
| --- | --- | --- | --- |
| `Matcher.extract` | `Matcher$` | bytes 150–262 | accepted |
| `Matcher.classify` | `Matcher$` | bytes 266–400 | accepted |

The baseline excludes generated case-class methods and nested `CaseDef`s. The
Rust regression parses and names the frozen source, confirms `Event` is the
sealed source trait, confirms `Created` and `Deleted` are distinct source case
classes, and runs both original method roots independently in two fresh source
sessions. Its exact two-row output is frozen in `rust-baseline.tsv`.

## Canonical source and binary evidence

Scala's typed tree assigns `Event` the two children `Created` and `Deleted`.
Both variants are final case classes with parents `Object`, `Event`,
`_root_.scala.Product`, and `Serializable`. Their selectors are class-owned
`_1: Int`. The generated `Created$.unapply(Created): Created` and
`Deleted$.unapply(Deleted): Deleted` methods are companion-owned. The
`scala/Product.tasty` fixture is available through the same authentic
`ClasspathSymbolResolver`; its fixture SHA-256 is
`ad9302597f965b7e218f34b83de00381b64d83487aae80bcc410d67233751e3c` used by the Rust baseline.

`rust-classinfo.tsv` preserves the current Rust canonical symbol/owner/type
evidence for both variants. The source AST and semantic index retain each
direct `Event` parent, but completed `ClassInfo` currently contains only
`Object` and `Event`; `Product` is absent despite the available resolver and
TASTy fixture. Each generated unapply has the correct companion owner and
variant input/result type. `_1` is present in the class scope, while completing
it currently reports `DeclarationContextMissing`. These are observations for
future scoped implementation work, not changes made by this baseline.

Both authentic whole-method Rust attempts stop at
`ExtractorPatternConstraintDeferred`: `extract` first reports the
`Created(id, _)` pattern at bytes 210–221; `classify` first reports the
`Created(id, _)` pattern at bytes 327–337. The failed method has no typed-root
index entry, and rollback removes the error's transient type IDs. The resolver
is attached to each attempt, so neither root fails because `Product` could not
be looked up by name.

## Bounded Scala classification matrix

The runner also recompiles twelve small frozen controls and compares each
diagnostic file byte-for-byte after whitespace normalization:

| Case | Result |
| --- | --- |
| `Event` | accepted |
| `NonSealedEvent` | accepted |
| `WrongConstructorLiteral` | rejected |
| `WrongResult` | rejected |
| `UnrelatedExtractorType` | rejected |
| `MissingParent` | rejected |
| `WrongParent` | rejected |
| `InvalidFieldShape` | rejected |
| `BinderEscape` | rejected |
| `DuplicateVariant` | accepted with warning |
| `NonExhaustive` | accepted with warning |
| `WrongAliasOwner` | accepted with warning |

The unrelated-extractor control declares `Text.unapply(String)` while matching
an `Event`; Scala 3.9 reports the case as unreachable and rejects the source.
The source and expected diagnostic digests are checked by the fixture
manifests. The matrix constrains the pinned Scala classification boundary; it
does not claim Rust compatibility for those controls.

## Reproduction and regression controls

Run the pinned oracle from the repository root with `SCALA39_ROOT` set to the
checkout at the recorded revision and `JAVA_HOME` set to Temurin 21.0.8+9:

```sh
crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/run
cargo +1.98.1 test -p dotty-typer --test source_sealed_adt_match_baseline \
  --locked -- --nocapture
```

The focused audit workflow runs both commands and uploads their logs. Its
existing Box 2/2, Product2 2/2, lexical closure 6/6, inherited member 4/4, and
protected stdlib 16 main + 15 analysis roots remain named regression controls.
The Rust baseline is also covered by focused formatting, lint, and frontend
boundary checks in the implementation validation record.
