# Scala 3.9.0 `Tuple2` applied constructor parent

This note records the bounded classpath behavior added for issue #2147. The
wire fixtures are the repository's pinned Scala 3.9.0 artifacts; their hashes
remain unchanged:

| Fixture | SHA-256 |
| --- | --- |
| `scala/Tuple2.tasty` | `beb51345df5c19be858f39953856aea0ff9d196b8c0f7ba98037bc36f15011bb` |
| `scala/$times$colon.tasty` | `a9bea01c727e9521bbc2c04b7c7f77de5e8e797f685614144f7b8edefdcc9139` |

In the authentic `Tuple2` declaration, parent 0 is an applied constructor
whose selected method is `<init>`. Its `NEW/APPLIEDTYPE` qualifier has a root
tycon and underlying shared target that must each resolve through the
supported package-qualified `TYPEREF` shape to `scala/*:`. Those identities
must match. Term references and references found only inside applied arguments
do not authenticate the constructor owner. The classloader
authenticates that exact shape and declaration before exposing the nominal
encoded identity `scala/$times$colon`. Parent 1 remains the independent
applied parent `scala/Product2[T1, T2]`; parents 2 and 3 remain `scala/Product`
and `scala/Serializable`. The mapping is limited to parent 0 of the authentic
`scala/Tuple2` request and does not define general operator-name encoding or
applied/higher-kinded type support.

The pinned declaration is selected from the direct `scala` package type
declaration with wire spelling `*:`. The same-spelled companion and unrelated
package declarations are not interchangeable. Duplicate matching declarations
fail as ambiguous. Neighboring `Tuple3` shapes do not receive this encoded
identity. A missing authenticated declaration fails closed; the loading
transaction restores the semantic store checkpoint, and retrying the same
loading session with the complete classpath succeeds. Repeated loading in the
same and fresh sessions preserves normalized parent identities and order.

The original #2134 `ForeignSameSpelledExtractor.scala` source fixture was run
unchanged through the existing Rust source-frontend harness on this
classpath-enabled revision. It next returns
`ExtractorResultMemberUnsupported { ..., issue: NotValueType }`. Consequently,
the foreign extractor owner/binder assertions are not reached yet. This is a
checkpoint for #2134, not a claim that its foreign-source behavior is complete.
