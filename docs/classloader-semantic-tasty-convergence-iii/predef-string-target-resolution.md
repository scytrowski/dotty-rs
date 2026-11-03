# `scala.Predef.String` target resolution (#2029)

This increment completes the one source-authentic `String` type alias already
projected from the Scala 3.9 `scala/Predef.tasty` module. The resolver still
returns the original Type-namespace alias owned by the canonical `Predef$`
ModuleClass. Its `SymbolInfo` now retains `AliasingBounds` around a
symbol-backed `TypeRef` to the real `java.lang.String` class. The alias and
class have distinct `SymbolId`s.

## Pinned evidence

- Integrated base: `73b27723e3897bf9059f6097db9fee440470c765`.
- Scala source revision: `777528f19a58e794c9954a42f433373472ec57f8`.
- `scala/Predef.tasty`: SHA-256
  `2faef7420109c4e44074ab0385dce2c31e751fd992621fd1de312eed6c362fb2`.
- Temurin JDK: `21.0.13+9-LTS`.
- `java.base.jmod`: SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- Original TASTy identities remain `Predef` object@107,
  `Predef$` ModuleClass@128, and direct `String` TYPEDEF@378. The selected
  RHS remains the address-indexed `java.lang.String` tree, not a classpath name
  inferred from `Predef.String`.

The classloader completes this target only while completing the validated
direct alias from `Predef$` TASTy. It loads the recorded `java/lang/String`
path through the same configured classpath and `LoadingSession`, requires a
class resource from `java.base.jmod`, confirms the resulting class has package
owner `java.lang`, and checks that the loaded class origin matches the selected
JMOD resource. The completed alias uses the existing `AliasingBounds` and
`TypeRef` model. No second Definitions or Packages authority is created.

## Lookup behavior and failure boundary

The type-member lookup on the canonical `Predef$` returns the unique indexed
TypeAlias. Term-namespace `String` still does not select it; a conflicting
same-named type member remains ambiguous. The declaration retains its original
owner, visibility, flags, and TASTy origin. Repeated lookup in one session
reuses its alias and class identities. Fresh resolver sessions produce the
same normalized owner, target, and `java.base.jmod` origin receipt.

Missing and malformed `java/lang/String` resources return an attributable
`ResolutionError::Malformed` and leave the resolver checkpoint, session
journal, semantic store, Predef module completion, and target-origin cache
unchanged. Restoring the genuine resource permits retry in the same store and
resolver. The rollback regression also exercises an enclosing TASTy completion
that fails after the target has materialized; retry completes the alias target
without reusing stale state.

The direct `TastyUnpickler::complete_symbol(378)` target-tree probe remains a
separate path. Its recorded JDK-backed failure at address 383 and no-JDK
package failure at address 385 are unchanged; resolving that path would require
broader nested-package/type-reference behavior outside this increment. The
original Animal/Dog audit remains signature-reference evidence only. This
change does not claim full `Animal.sound` or `Dog.sound` completion.

The frozen 14-unit / 126-row reports and historical
`predef-alias-audit.tsv` remain unchanged. The current target assertions live
in focused resolver and classloader transaction tests instead.
