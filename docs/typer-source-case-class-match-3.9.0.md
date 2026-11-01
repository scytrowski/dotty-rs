# Source case-class extractor matches (Scala 3.9.0)

The source fixture `crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/Positive.scala` remains byte-for-byte unchanged (SHA-256 `df57333192eb934169f06a07873c2e1ba88c9159404f60e5bfe33b3d016e215c`). Its `unwrap` and `classify` roots remain source trees 18 and 36. The fixture is pinned to Scala revision `777528f19a58e794c9954a42f433373472ec57f8`.

The typer integration test resolves the real `scala.Product` TASTy from `crates/dotty-tasty/tests/fixtures/scala3-library`, using the existing `ClasspathSymbolResolver` and JDK module path for its Java parent. The TASTy fixture manifest identifies Scala library 3.9.0 at the pinned source revision; `scala/Product.tasty` has SHA-256 `ad9302597f965b7e218f34b83de00381b64d83487aae80bcc410d67233751e3c`. The test checks that `Box` inherits a `Product` symbol with `SymbolOrigin::Tasty`; it does not synthesize `Product` or identify symbols by spelling alone.

Both original methods now type as complete `DefDef`s. The test verifies their signatures, typed `Match` RHS, selector, ordered `CaseDef`s, source-to-typed mappings, actual generated `Box.unapply` symbol, and companion qualifier. `unwrap` has one `UnApply` with a case-local `Int` binder used by its body. `classify` preserves its two cases, checks the first nested literal against the `Int` component prototype, and binds the second case's `value` locally. Its case results join to `Int` through the existing expression join. Fresh sessions and reordered top-level declarations retain the same semantic result.

Focused negative tests reject a Boolean component, zero and two pattern arguments, a non-case class with no extractor, a result-type mismatch, a wrong selector type, and a reference to a binder from an earlier case. A late method-result error after extractor and binder processing restores the semantic store, typed arena, mappings, and generated `unapply` completion state; retry returns the same failure. Broader extractor-owner and generated-identity cases remain covered by the existing `patterns.rs` regressions and the Scala 3.9 source-case-class fixture matrix.

| Root | Before #1937 (after Product projection) | After #1937 |
| --- | --- | --- |
| `unwrap` (`Positive.scala#tree18`, bytes 64–136) | `TypeNameNotFound(Product)`; no typed root | Complete `DefDef` → `Match` → one `CaseDef` → generated `Box.unapply` `UnApply`; local `value: Int`; result `Int` |
| `classify` (`Positive.scala#tree36`, bytes 140–237) | `TypeNameNotFound(Product)`; no typed root | Complete `DefDef` → `Match` → ordered two `CaseDef`s; nested `0: Int`, second-case local `value: Int`; result `Int` |

The exact machine-readable two-root receipt is `crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/rust-before-after.tsv`. This increment does not add general reference LUBs, exhaustivity checking, pattern syntax, generated identities, JVM lowering, or TASTy emission.

The pinned Scala oracle runner `crates/dotty-typer/tests/fixtures/source-case-class-match-3.9.0/run` passes with Scala revision `777528f19a58e794c9954a42f433373472ec57f8`, the eight SHA-256-verified Scala 3.9.0 runtime artifacts, and Temurin 21.0.8+9. It reports matching whole-source snapshots for `Positive.scala` and `SameSpelledExtractor.scala` and a matching bounded extractor acceptance matrix.
