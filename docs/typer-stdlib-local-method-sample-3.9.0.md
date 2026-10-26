# Scala standard-library local-method sample, Scala 3.9.0

This focused sample tests the #1566 objective directly: whether the source typer fully types a method definition already present in the Scala standard library. It uses the existing full-local-method audit. A method counts as typed only when the complete local `DefDef` has a typed-tree mapping in `SourceTypedIndex`; typing just its RHS expression does not count.

Inputs are pinned to Scala revision `777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and JDK 21. The sample includes seven library files with nested local method definitions:

- `scala/StringContext.scala`
- `scala/collection/IndexedSeqView.scala`
- `scala/collection/StringParsers.scala`
- `scala/collection/mutable/RedBlackTree.scala`
- `scala/reflect/ClassManifestDeprecatedApis.scala`
- `scala/util/Random.scala`
- `scala/util/Using.scala`

## Result

The audit attempted 36 local method definitions. **0 of 36 were fully typed.** All seven files parsed and were named successfully. Two normalized focused runs were byte-identical.

First-blocker counts:

| First blocker | Methods |
| --- | ---: |
| `ApplicationCalleeNotMethod` | 11 |
| `TypeNameNotFound` | 7 |
| `TermNameNotFound` | 5 |
| `SymbolResolution` | 3 |
| `ConstructorResultTypeMismatch` | 3 |
| `ApplicationArgumentConformanceUnsupported` | 2 |
| `MemberLookup` | 2 |
| `UnsupportedTypeTree::Annotated` | 2 |
| `MemberNotFound` | 1 |

This sample found no fully typed method, so it does not satisfy #1566 or demonstrate semantic progress. It does establish that the sampled standard-library helpers still encounter the existing typer blockers. The result is limited to these seven files and is not a claim about the entire library.

Reproduce with:

```sh
bash tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-stdlib-local-method-sample-3.9.0.tsv \
  --verify-determinism /tmp/typer-stdlib-local-method-sample.md
```
