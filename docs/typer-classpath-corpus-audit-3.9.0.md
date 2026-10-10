# Typer classpath corpus audit: Scala 3.9.0

The report is generated from two byte-identical runs of the pinned Scala 3.9.0 audit. It records first blockers for local method typing, isolated Match-case probes, PatDef attempts, and structural inventories. A moved first blocker is not semantic success. The source corpus, parser/namer setup, Scala revision, JDK feature release 26, and classpath remain pinned.

Run `tools/typer-classpath-corpus-audit/run` with the environment documented in `docs/typer-v0.1-compatibility.md` to regenerate this report.

## Historical #767–#771 expression sprint snapshot

At the #772 snapshot, first blockers for term prefix operators and term annotations had moved to zero. These historical measurements are retained here for context; current values and ranking come from the report below.

| First blocker | Before #767 | At #772 snapshot |
| --- | ---: | ---: |
| `UnsupportedExpression::PrefixOp` | 66 / 23 files | 0 / 0 |
| `UnsupportedExpression::Annotated` (term expressions) | 64 / 14 files | 0 / 0 |

## #826–#829 source function support

The pre-#826 baseline at `89d84f41444a2a03a6a5bd39cf45176ca894a03b` and the post-#829 audit were run against the same pinned Scala 3.9.0 corpus, JDK 21, and classpath. The post-#829 audit ran twice; normalized reports matched byte-for-byte. It tracks the same 25 local methods by source path, line, and name. The 13 expression cases now split into 9 explicit `UnsupportedFunctionLiteralParameter` deferrals and 4 `ImportQualifierNotFound` blockers; none fully typed. The 12 ordinary function-type cases now reach `SymbolResolution` in 8 files; none fully typed. `FunctionWithMods` remains 13 / 2 files. These are first-blocker movements, not semantic success. The corpus still typed zero local methods overall and materialized zero external member symbols.

| Baseline family | New first blocker | Count / files | Representative methods |
| --- | --- | ---: | --- |
| `UnsupportedExpression::Function` | `ImportQualifierNotFound` | 4 / 1 | compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:737:refersTo, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:752:removeSingleton, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:754:mapArg, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:758:elim |
| `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` | 9 / 1 | compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:710:factoryManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:718:singletonManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:721:synthArrayManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:727:synthWildcardManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:731:synthArgManifests |
| `UnsupportedTypeTree::Function` | `SymbolResolution` | 12 / 8 | compiler/src/dotty/tools/backend/jvm/BTypes.scala:671:ifInit, compiler/src/dotty/tools/backend/jvm/BTypes.scala:673:isJLO, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2387:genArgs, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2388:genArgsAsClassCaptures, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:3386:genScalaArgs |

### Same-method comparison

| Method from the pre-#826 first-blocker set | Baseline family | First blocker after #829 |
| --- | --- | --- |
| `compiler/src/dotty/tools/backend/jvm/BTypes.scala:671:ifInit` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/backend/jvm/BTypes.scala:673:isJLO` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2387:genArgs` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2388:genArgsAsClassCaptures` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:3386:genScalaArgs` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:3387:genJSArgs` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/dotc/core/Denotations.scala:310:argStr` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/dotc/core/Types.scala:3448:normalize` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:737:refersTo` | `UnsupportedExpression::Function` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:752:removeSingleton` | `UnsupportedExpression::Function` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:754:mapArg` | `UnsupportedExpression::Function` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:758:elim` | `UnsupportedExpression::Function` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/parsing/Parsers.scala:3395:maybeAscription` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/dotc/transform/PostTyper.scala:366:unusable` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/dotc/typer/ProtoTypes.scala:409:isPoly` | `UnsupportedTypeTree::Function` | `SymbolResolution` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:710:factoryManifest` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:718:singletonManifest` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:721:synthArrayManifest` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:727:synthWildcardManifest` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:731:synthArgManifests` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:741:canManifest` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:748:synthManifest` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:770:manifestOfType` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:774:synthesize` | `UnsupportedExpression::Function` | `UnsupportedFunctionLiteralParameter` |
| `library/src/scala/util/control/Exception.scala:416:fun` | `UnsupportedTypeTree::Function` | `SymbolResolution` |

The detailed rows are emitted from the same local-method attempts as the aggregate audit; the generated raw report below preserves all bucket counts and paths. `Typed` would mean the local method received a typed-tree mapping; none of these 25 rows did.

## #802–#806 local extension hardening

The initial local-extension blocker was 25 occurrences in 5 files. The current audit splits extension-specific failures from ordinary local method signatures. It attempted 3,297 local method definitions and fully typed zero; moved failures are not counted as semantic success. Focused tests cover a fully typed local extension call, nominal-member precedence, local shadowing, unsupported receiver/signature forms, rollback, and bounded ambiguity.

| Focused extension failure bucket | Count / files | Representative paths |
| --- | ---: | --- |
| `LocalExtensionGroupShapeDeferred` | 3 / 1 | compiler/src/dotty/tools/dotc/typer/Applications.scala |
| `LocalExtensionReceiverTypeNotFound` | 1 / 1 | compiler/src/dotty/tools/dotc/core/Flags.scala |
| `LocalExtensionSignatureDeferred::dependent result types` | 1 / 1 | compiler/src/dotty/tools/dotc/reporting/MessageRendering.scala |

Receiver or ordinary-argument mismatches remain non-applicable candidates and preserve `MemberNotFound`; the audit does not relabel those as successful typing. Multiple viable same-name local extensions produce `OverloadedSelectionDeferred` in a focused regression. Primitive definitions have no modeled member scopes. If nominal lookup cannot inspect a primitive member index, local extension resolution is not attempted and the original `MemberLookup` error is retained; fallback is available only after a completed index confirms that the ordinary name has no member. External member materialization remains zero.

### Current top ten Typer-owned semantic blockers

The ranked list below excludes parser/namer and classpath-resolution failures, and excludes the downstream `NoSuccessfulEnclosingMethodTyping` counter. Counts and representative paths are in `top_semantic_gaps` in the raw report.

| Rank | First blocker | Count / files |
| ---: | --- | ---: |
| 1 | `AnonymousClassInstantiationDeferred` | 32 / 15 |
| 2 | `LocalBlockDeclarationDeferred::val/var definition` | 21 / 8 |
| 3 | `UnsupportedTypeTree::FunctionWithMods` | 13 / 2 |
| 4 | `LocalBlockDeclarationDeferred::type definition` | 11 / 4 |
| 5 | `SymbolSourceKindMismatch` | 11 / 3 |
| 6 | `UnsupportedExpression::ParsedTry` | 11 / 6 |
| 7 | `MissingDeclaredType` | 10 / 3 |
| 8 | `LocalBlockDeclarationDeferred::module definition` | 9 / 2 |
| 9 | `TypedPatternRuntimeTestDeferred` | 9 / 2 |
| 10 | `UnsupportedFunctionLiteralParameter` | 9 / 1 |

### Recommended next Typer sprint

After #902, the local signature profile contains 0 first-blocker observations, down from 14 affected methods across 3 files in the #899 baseline. The 8 by-name observations moved past the signature guard in #900; the 6 parameter-modifier observations (one direct origin and five inherited siblings in `MegaPhase.scala`) moved past it in #902 by accepting the already-modeled `Inline` parameter flag on local `inline` methods. This blocker movement is not evidence that all six enclosing methods type; inspect their current first blockers and the `typed_local_defdefs` count below. #901 removes the bounded by-name application blocker in the focused local-call fixture; the two corpus methods originating from by-name signatures remain behind `ImportQualifierNotFound`. Inline expansion and dependent-result signatures remain out of scope. Recommend #903 to harden these signature paths and refresh the Typer ranking.

### #899–#902 local method signature profile

The profile counts local methods whose first blocker is a local signature failure. `direct_origins` identifies methods carrying that signature feature; `inherited_methods` are sibling methods attributed to the same enclosing failure. The #899 baseline had 14 affected methods across 3 files; after #900 and #902, none remain at this blocker. This deterministic supplemental audit was run twice with JDK feature release 26 and the pinned Scala 3.9.0 source/artifact inputs; normalized output matched byte-for-byte. Full method typing is reported separately.

| Feature payload | Affected methods | Files | Distinct methods | Direct origins | Inherited methods | Blocker origins |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| (none) | 0 | 0 | 0 | 0 | 0 | 0 |

Residual local signature first-blocker rows appear below with their source path, line/span, tree index, feature payload, signature shape, blocker origin, and direct/inherited attribution. No rows remain at this blocker after #902.

```text
(none; 0 occurrences)
```

#### #902 selected inline-parameter cohort

The six rows from the #899 `parameter modifiers` feature profile are tracked by their pinned source-tree indexes below. They are local `inline` methods; a moved blocker is not full method typing.

| Baseline local method tree | Current first blocker |
| --- | --- |
| `compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1125` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1440` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1224` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1301` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=2407` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=2116` | `ImportQualifierNotFound` |

### #900 by-name downstream blocker inspection

The two direct by-name origins are measured individually after signature support. In the whole-file audit, both now reach `ImportQualifierNotFound` as their first blocker. No matching `ByNameApplicationParameterDeferred` observation is emitted for these methods; their application sites are still behind the enclosing method blocker. At the #900 snapshot the `ByNameCall.scala` fixture reached the application deferral boundary; the #901 follow-up below removes it for that bounded local-call shape.

| Corpus method | Result shape | First blocker after #900 | Application sites reached by audit |
| --- | --- | --- | --- |
| `SymUtils.scala::instantiateCFT` | Explicit | `ImportQualifierNotFound` | No; recursive RHS and `returnProto` calls remain behind the enclosing blocker. |
| `Typer.scala::cases` | Inferred | `ImportQualifierNotFound` | No; calls from `typedTyped` remain behind the enclosing blocker. |

### #901 by-name application follow-up

The focused local `ByNameCall.scala` fixture previously produced one `ByNameApplicationParameterDeferred` observation. After #901 it produces zero such failures and its one local method types successfully. In the pinned corpus, zero applications from the two direct by-name origins reached application typing before or after #901: both remain blocked by `ImportQualifierNotFound` in their enclosing methods. Thus the corpus count is 0 moved past the application blocker; the focused regression demonstrates the implemented call behavior.


### Historical #825 next-sprint candidates (pre-#856)

| Candidate | Smallest useful slice and owner | Prerequisites / reuse | Non-goals |
| --- | --- | --- | --- |
| `MissingDeclaredType` (34 / 15, historical) | #825 proposed immutable inferred class fields (14 occurrences / 7 files); implemented in #854–#856 and audited below. | Reused declaration source contexts, expression typing, widening, and completion rollback. | Current audit distinguishes first-blocker movement from completed semantics. |
| `AnonymousClassInstantiationDeferred` (32 / 15) | Support one anonymous `new` with one concrete parent in `typer/expression/new.rs`. | Reuse ordinary `New` typing and parent projection; define stable anonymous symbol ownership and class info. | Closure capture, refinement synthesis, and general anonymous-class members. |
| `UnsupportedSingletonReference` (historical 22 / 1) | #880 profiled the baseline; #881–#883 now project, relate, and preserve supported literal constants. | Reuse `Type::Constant`, literal expression typing, bounded relations, expected adaptation, and stable-prefix validation. | Arbitrary paths, unstable prefixes, and path-dependent relation redesign. |
| `LocalBlockDeclarationDeferred::val/var definition` (21 / 8) | Split remaining cases by PatDef root/binder shape in `typer/expression/blocks.rs`. | Reuse transactional PatDef lowering, local binders, and assignment support. | General destructuring and reopening supported PatDef forms. |
| `LocalMethodSignatureDeferred` (0 current; after #902) | #900 moved 8 by-name and #902 moved 6 inline-parameter observations past the signature guard. | Reuse `source_method_flags` and the shared method-signature builder; local parameter symbols retain `Inline`. | Inline expansion; dependent results; erased parameter support or new method inference. |

## Historical #825 MissingDeclaredType producer profile (pre-#856)

Before #856, the 34 first blockers across 15 files were all attributed to source `ValDef` declarations with a right-hand side and a zero-width synthetic inferred `TypeTree`. They split into 24 immutable values and 10 mutable variables; all have semantic `Field` identity. Source `val` versus `var` comes from the AST modifier and is cross-checked against the semantic `MUTABLE` flag; `SymbolKind::Field` alone does not preserve that distinction. The owner kind is `Class` for 24 occurrences and `ModuleClass` for 10. Four immutable module-class values carry `Inline`. Counts are downstream local-method first blockers, so one source declaration can appear more than once. The baseline profile is frozen here; current profile values are listed separately below.

| Source declaration bucket | Occurrences / files | Representative paths |
| --- | ---: | --- |
| Total (17 distinct declarations) | 34 / 15 | — |
| `value::Field::owner=Class::synthetic inferred TypeTree::rhs=true::modifiers=::semantic_mutable=false` | 14 / 7 | compiler/src/dotty/tools/dotc/core/TypeErrors.scala, compiler/src/dotty/tools/dotc/inlines/Inliner.scala, compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala, compiler/src/dotty/tools/dotc/reporting/Profile.scala, compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala, compiler/src/dotty/tools/dotc/transform/Bridges.scala, compiler/src/dotty/tools/io/FileWriters.scala |
| `variable::Field::owner=Class::synthetic inferred TypeTree::rhs=true::modifiers=Var::semantic_mutable=true` | 10 / 6 | compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala, compiler/src/dotty/tools/dotc/reporting/Message.scala, compiler/src/dotty/tools/dotc/typer/Applications.scala, compiler/src/dotty/tools/dotc/util/WeakHashSet.scala, library/src/scala/collection/Iterator.scala |
| `value::Field::owner=ModuleClass::synthetic inferred TypeTree::rhs=true::modifiers=::semantic_mutable=false` | 6 / 2 | compiler/src/dotty/tools/dotc/core/NamerOps.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala |
| `value::Field::owner=ModuleClass::synthetic inferred TypeTree::rhs=true::modifiers=Inline::semantic_mutable=false` | 4 / 1 | compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala |

### Current #858 MissingDeclaredType profile

| Source declaration bucket | Occurrences / files | Representative paths |
| --- | ---: | --- |
| Total (3 distinct declarations) | 10 / 3 | — |
| `value::Field::owner=ModuleClass::synthetic inferred TypeTree::rhs=true::modifiers=::semantic_mutable=false` | 6 / 2 | compiler/src/dotty/tools/dotc/core/NamerOps.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala |
| `value::Field::owner=ModuleClass::synthetic inferred TypeTree::rhs=true::modifiers=Inline::semantic_mutable=false` | 4 / 1 | compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala |

### Family assessment

| Producer family | Smallest useful slice | Infrastructure and reuse | Main risks and non-goals |
| --- | --- | --- | --- |
| Immutable `Class` fields (14 / 7) | Implemented in #854–#856; this report measures whether the corpus shapes pass the bounded path. | Field initializer context, ordinary RHS typing, widening, recursion guard, cache, and rollback are now covered by focused tests. | Full-corpus `ImportQualifierNotFound` movement remains a blocker; do not count movement as semantic success. |
| Mutable `Class` fields (10 / 6) | Implemented in #858; the pinned audit tracks all 10 source-method attempts individually. | Reuses the initializer context, RHS typing, widening, recursion guard, transactional completion, and assignment relation. | Corpus first-blocker movement is not semantic completion; module-class fields remain separate. |
| Ordinary `ModuleClass` fields (6 / 2) | Infer one non-inline module member value after class fields. | Reuse field type inference with a module-class owner and the namer-provided lexical context. | Singleton initialization and module cycles differ from per-instance class initialization. Keep module values out of the first slice. |
| Inline `ModuleClass` fields (4 / 1) | Add inline values as a separate follow-up after ordinary module values. | Reuse only the proven module initializer path and existing inline flags. | Compile-time constant and inline expansion rules add constraints. Do not assume ordinary runtime RHS typing is sufficient. |


`complete_symbol` reuses an already-complete symbol, and source type projection reuses a cached type-index entry before projection. #854–#856 routed eligible immutable class fields through the initializer context, normal RHS typing, widening, recursion protection, and transactional publication. #858 extends that same bounded path to mutable class fields.

### Historical #825 recommendation (pre-#856)

The recommendation above is the historical #825 plan; immutable `Class` field inference landed in #854–#856, followed by mutable fields in #858.

## Historical #856–#857 immutable class field inference audit

At the #856 snapshot, the #825 baseline had 34 `MissingDeclaredType` first-blocker occurrences in 15 files. Immutable class fields accounted for 14 occurrences across 7 files and no longer stopped at `MissingDeclaredType`; 20 occurrences in 8 files remained, including 10 mutable `Class` occurrences across 6 files. The immutable field and method outcomes from that snapshot are retained below as historical measurements.

| Baseline immutable `Class` field | Weighted occurrences | Direct completion outcome |
| --- | ---: | --- |
| `TypeErrors.scala:cycleSym` | 1 | `ImportQualifierNotFound` |
| `Inliner.scala:thisProxy` | 1 | `ImportQualifierNotFound` |
| `ReplPrinter.scala:debugPrint` | 1 | `ImportQualifierNotFound` |
| `Profile.scala:pinfo` | 5 | `ImportQualifierNotFound` |
| `Rewrites.scala:pbuf` | 2 | `ImportQualifierNotFound` |
| `Bridges.scala:bridgesScope` | 3 | `ImportQualifierNotFound` |
| `FileWriters.scala:isWindows` | 1 | `ImportQualifierNotFound` |

The following table tracks the 14 baseline local-method attempts independently from direct field completion. Method tree indexes are pinned to the same source revision and each row is asserted in the ignored audit test.

| Baseline local-method attempt | First blocker at #857 |
| --- | --- |
| `compiler/src/dotty/tools/dotc/core/TypeErrors.scala#tree=695` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/inlines/Inliner.scala#tree=1538` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala#tree=396` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=470` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=531` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=549` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=660` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=811` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala#tree=235` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala#tree=292` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/Bridges.scala#tree=204` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/Bridges.scala#tree=212` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/transform/Bridges.scala#tree=247` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/io/FileWriters.scala#tree=1156` | `ImportQualifierNotFound` |

Each direct immutable field-completion attempt stops at `ImportQualifierNotFound` while resolving an import from the field source context. In the pinned sources these imports are at file scope before the field declarations; this does not establish that the corresponding RHS was fully typed. The 14 local-method outcomes are first blockers reached through those methods, not successful RHS inferences.

## #858 mutable class field inference audit

The Scala 3.9.0 audit ran twice against revision `777528f19a58e794c9954a42f433373472ec57f8`; normalized reports matched byte-for-byte. All 10 pre-change mutable-class occurrences across 6 files are retained as individual local-method rows below, alongside direct completion outcomes for all 7 distinct fields (two fields are declared in `Scanners.scala`). `MissingDeclaredType` fell from 20 occurrences in 8 files at the #856 snapshot to 10 in 3 files; the remaining records are 6 ordinary `ModuleClass` occurrences across 2 files and 4 inline `ModuleClass` occurrences in 1 file. These are first-blocker movements, not proof that all field RHS expressions or enclosing methods type successfully.

### Direct mutable field completion

| Baseline mutable field | Weighted occurrences | Direct completion outcome |
| --- | ---: | --- |
| `CheckCaptures.scala:curEnv` | 2 | `ImportQualifierNotFound` |
| `Scanners.scala:allowLeadingInfixOperators` | 1 | `completed` |
| `Scanners.scala:skipping` | 1 | `completed` |
| `Message.scala:disambi` | 2 | `completed` |
| `Applications.scala:typedArgBuf` | 1 | `ImportQualifierNotFound` |
| `WeakHashSet.scala:table` | 1 | `TypeNameNotFound` |
| `Iterator.scala:currentHasNextChecked` | 2 | `completed` |

| Baseline mutable-field method attempt | Current first blocker |
| --- | --- |
| `compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala#tree=5089` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala#tree=5879` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/parsing/Scanners.scala#tree=1474` | `MemberLookup` |
| `compiler/src/dotty/tools/dotc/parsing/Scanners.scala#tree=998` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Message.scala#tree=291` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/reporting/Message.scala#tree=349` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/typer/Applications.scala#tree=4248` | `ImportQualifierNotFound` |
| `compiler/src/dotty/tools/dotc/util/WeakHashSet.scala#tree=708` | `TypeNameNotFound` |
| `library/src/scala/collection/Iterator.scala#tree=3805` | `UnsupportedTypeTree::Annotated` |
| `library/src/scala/collection/Iterator.scala#tree=3865` | `UnsupportedTypeTree::Annotated` |

The top-ranked remaining gap is `AnonymousClassInstantiationDeferred` (32 occurrences across 15 files), but its implementation risk and anonymous-identity work are larger than the literal singleton slice. #880 traced 22 `UnsupportedSingletonReference` observations to one `true` declaration. #881 implemented literal-to-`Type::Constant` source projection, #882 added exact constant relations, and #883 preserves constants for singleton expected types. Keep `null`, class literals, arbitrary paths, unstable prefixes, and path-dependent relation changes deferred.

## #880 literal singleton blocker profile

The #880 baseline audit ran twice at revision `777528f19a58e794c9954a42f433373472ec57f8`; normalized output matched byte-for-byte. Its 22 `UnsupportedSingletonReference` first-blocker observations all pointed to one source singleton tree and one enclosing declaration. The current values are reported separately under #881 below.

| Profile measure | Result |
| --- | ---: |
| total first blockers | 22 |
| profile entries | 22 |
| distinct singleton source trees | 1 |
| distinct enclosing declarations | 1 |
| distinct reference shapes | 1 |

All 22 were repeated observations of `val actionable: true = true` in `compiler/src/dotty/tools/dotc/transform/CheckUnused.scala`: singleton tree 2904, reference tree 2903, source span `[27444, 27448)`, and reference shape `Literal(Boolean(true))`. This is one declaration repeated through distinct methods, not 22 independent singleton declarations. Scala 3.9 and the dotty-rs parser retain `SingletonTypeTree(reference = Literal(...))` for literal references. After #881, supported literal forms project to exact `Type::Constant` values.

### Narrow follow-up boundaries

| Concern | Recommended boundary |
| --- | --- |
| Source type projection | Implemented in #881: supported source literals project directly to `Type::Constant` with the exact payload. |
| Type relation | Implemented in #882: compare exact supported constant payloads and relate constants to modeled primitive/Unit types. String constants compare exactly; String widening remains unsupported without a canonical source type. |
| Expected adaptation | Implemented in #883: preserve exact constants for singleton expected types, including singleton alternatives in unions; ordinary expectations still use the widened type. |
| Explicit deferrals | Keep `null`, class literals, arbitrary paths, unstable prefixes, and path-dependent relation changes unsupported. |

## #881–#884 literal singleton hardening audit

The pinned Scala 3.9.0 audit compares the same 22 #880 method observations after literal projection, constant relations, expected adaptation, and hardening. Each baseline method is required to remain present, and the run fails if any still stops at `UnsupportedSingletonReference`. A moved first blocker is not full method typing. Two normalized audit runs must match byte-for-byte.

| Projection measure | Result |
| --- | ---: |
| baseline observations | 22 |
| no longer first blocked by singleton projection | 22 |
| remaining UnsupportedSingletonReference | 0 |

### Baseline outcome classification

| Outcome | Count |
| --- | ---: |
| Deeper Typer semantic blocker | 0 |
| Resolution or classpath blocker | 22 |
| Residual singleton-specific blocker | 0 |
| Fully typed baseline methods | 0 |

Resolution or classpath first blockers do not show whether the corpus attempts reached singleton projection or expected-type adaptation. Focused fixtures establish those paths separately.

### Singleton source inventory

| Reference category | Singleton trees | Files | Source files |
| --- | ---: | ---: | --- |
| `literal` | 39 | 13 | compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/core/Periods.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/dotc/transform/CheckUnused.scala |
| `this` | 987 | 117 | compiler/src/dotty/tools/backend/jvm/opt/FifoCache.scala, compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala |
| `identifier` | 930 | 122 | compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala, compiler/src/dotty/tools/dotc/core/MacroClassLoader.scala, compiler/src/dotty/tools/dotc/core/NamerOps.scala |
| `selection` | 44 | 17 | compiler/src/dotty/tools/dotc/Run.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Access.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Constant.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Signature.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Tree.scala |
| `unsupported` | 12 | 1 | library/src/scala/util/Try.scala |

#### Reference shapes, including unsupported AST nodes

| Shape | Occurrences | Files | Source files |
| --- | ---: | ---: | --- |
| `Ident` | 930 | 122 | compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala, compiler/src/dotty/tools/dotc/core/MacroClassLoader.scala, compiler/src/dotty/tools/dotc/core/NamerOps.scala |
| `Literal(Boolean)` | 33 | 13 | compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/core/Periods.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/dotc/transform/CheckUnused.scala |
| `Literal(Int)` | 6 | 2 | library/src/scala/NamedTuple.scala, library/src/scala/Tuple.scala |
| `Other(Annotated)` | 12 | 1 | library/src/scala/util/Try.scala |
| `Select` | 44 | 17 | compiler/src/dotty/tools/dotc/Run.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Access.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Constant.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Signature.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Tree.scala |
| `This` | 987 | 117 | compiler/src/dotty/tools/backend/jvm/opt/FifoCache.scala, compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala |

| Current first blocker for baseline methods | Count |
| --- | ---: |
| `ImportQualifierNotFound` | 22 |

The raw profile below lists the current first blocker for each of the 22 pinned method trees. After #883, the former `LocalValueTypeMismatch` blocker moved to `ImportQualifierNotFound`; these observations show where typing stops and do not assert that the methods completed typing.

## Previous type projection snapshot

The #738 type-projection measurements remain historical: 28 unsupported type-tree first blockers in 12 files. They are not current counts; current `type_tree_forms` and `unsupported_type_tree_failures` below are the refreshed values.

## Full raw audit output (JDK feature release 26)

The raw audit below corresponds to the pinned source state processed by this run. Its aggregate counts and rankings should be read together with the source-state and JDK provenance in this report.

```text
scala_revision=777528f19a58e794c9954a42f433373472ec57f8
files=1236
parser_failed_files=0
namer_failed_files=0
file_read_failed_files=0
file_read_failed_paths=
recovered_parser_files=0
audit_v1_comparison:
  files_attempted=1236 (delta=+0)
  local_declarations=22713 (delta=-505)
  local_methods=3297 (delta=-481)
  typed_local_methods=0 (delta=+0)
  ImportQualifierNotFound=2876 (delta=+830)
  UnsupportedExpression_total=25 (delta=-547)
  LocalBlockDeclarationDeferred=41 (delta=-231)
  NoSuccessfulEnclosingMethodTyping=34 (delta=-449)
audit_581_feature_comparison:
  UnsupportedExpression::Parens=0 (baseline=146, delta=-146)
  UnsupportedExpression::InfixOp=0 (baseline=146, delta=-146)
  LocalBlockDeclarationDeferred::import=0 (baseline=139, delta=-139)
local_definitions=22713
local_val_defs=19067
local_def_defs=3297
local_imports=227
local_type_defs=35
local_objects=34
local_classes=28
local_pattern_bindings=25
expression_forms:
  Ident=250525
  Select=117733
  Apply=87453
  Block=42730
  InfixOp=39381
  If=16290
  Parens=15385
  Match=7379
  Assign=6004
  Function=4924
  New=4831
  PrefixOp=4567
  TypeApply=4219
  InterpolatedString=3179
  Tuple=1334
  While=1303
  Throw=792
  Return=693
  ForDo=656
  ParsedTry=507
  Annotated=401
  Typed=372
  ForYield=161
  InlineIf=4
  MacroTree=4
  InlineMatch=2
  PolyFunction=0
  PostfixOp=0
  Try=0
prefix_operator_forms:
  operator="!" count=4364
  operator="+" count=1
  operator="-" count=105
  operator="~" count=97
type_tree_forms:
  Annotated=1403
  AppliedTypeTree=15684
  ByNameTypeTree=456
  ContextBoundTypeTree=808
  Function=1262
  FunctionWithMods=971
  Ident=100586
  InfixOp::&=252
  InfixOp::<other>=71
  InfixOp::|=1087
  LambdaTypeTree=160
  MatchTypeTree=0
  Parens=58
  PostfixOp::*=181
  PostfixOp::<other>=0
  RefinedTypeTree=22
  Select=9913
  SingletonTypeTree=831
  Tuple=1345
  TypeBoundsTree=7921
  TypeTree=0
type_tree_form_files:
  Annotated=115 [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/printing/PlainPrinter.scala, compiler/src/dotty/tools/dotc/transform/Constructors.scala, compiler/src/dotty/tools/dotc/transform/init/Objects.scala]
  AppliedTypeTree=746 [compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala]
  ByNameTypeTree=161 [compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/PostProcessorFrontendAccess.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/jvm/opt/Inliner.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala]
  ContextBoundTypeTree=41 [compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/config/Settings.scala, compiler/src/dotty/tools/dotc/core/OrderingConstraint.scala, compiler/src/dotty/tools/dotc/core/TypeEval.scala, compiler/src/dotty/tools/dotc/printing/Formatting.scala]
  Function=233 [compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypeLoader.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala]
  FunctionWithMods=128 [compiler/src/dotty/tools/backend/jvm/PostProcessorFrontendAccess.scala, compiler/src/dotty/tools/dotc/Run.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/cc/CaptureOps.scala]
  Ident=1100 [compiler/src/dotty/tools/FatalError.scala, compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala]
  InfixOp::&=60 [compiler/src/dotty/tools/dotc/transform/LambdaLift.scala, compiler/src/scala/quoted/runtime/impl/QuoteMatcher.scala, compiler/src/scala/quoted/runtime/impl/QuotesImpl.scala, compiler/src/scala/quoted/runtime/impl/printers/Extractors.scala, compiler/src/scala/quoted/runtime/impl/printers/SourceCode.scala]
  InfixOp::<other>=15 [compiler/src/dotty/tools/dotc/printing/Formatting.scala, library/src/scala/NamedTuple.scala, library/src/scala/Option.scala, library/src/scala/Tuple.scala, library/src/scala/collection/IterableOnce.scala]
  InfixOp::|=227 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala]
  LambdaTypeTree=49 [library/src/scala/NamedTuple.scala, library/src/scala/Tuple.scala, library/src/scala/collection/BuildFrom.scala, library/src/scala/collection/Factory.scala, library/src/scala/collection/IndexedSeq.scala]
  MatchTypeTree=0 []
  Parens=22 [compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/quoted/PickledQuotes.scala, compiler/src/dotty/tools/dotc/typer/RefChecks.scala, compiler/src/dotty/tools/dotc/typer/TypeAssigner.scala, compiler/src/dotty/tools/scripting/ScriptingDriver.scala]
  PostfixOp::*=85 [compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/sjs/ScopedVar.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/cc/RetainingAnnotation.scala]
  PostfixOp::<other>=0 []
  RefinedTypeTree=10 [compiler/src/dotty/tools/dotc/core/Symbols.scala, compiler/src/dotty/tools/dotc/transform/CompleteJavaEnums.scala, compiler/src/dotty/tools/dotc/transform/MacroAnnotations.scala, library/src/scala/collection/MapView.scala, library/src/scala/collection/generic/IsIterable.scala]
  Select=383 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala]
  SingletonTypeTree=159 [compiler/src/dotty/tools/backend/jvm/opt/FifoCache.scala, compiler/src/dotty/tools/dotc/Run.scala, compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala]
  Tuple=252 [compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala]
  TypeBoundsTree=525 [compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypeLoader.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/jvm/GenericSignatureVisitor.scala]
  TypeTree=0 []
parser_diagnostics:
local_defdefs=3297
typed_local_defdefs=0
local_patdefs:
  total=790
  root_shapes:
    Alternative=0 files=[]
    Apply / extractor-looking=178 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/jvm/PostProcessor.scala]
    Bind=24 files=[compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/TreeMapWithTrackedStats.scala, compiler/src/dotty/tools/dotc/cc/Setup.scala, compiler/src/dotty/tools/dotc/core/ConstraintHandling.scala, compiler/src/dotty/tools/dotc/core/Denotations.scala]
    Ident=89 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/Constructors.scala, compiler/src/dotty/tools/dotc/transform/FirstTransform.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala]
    InfixOp=42 files=[compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/debug/ExpressionCompiler.scala, compiler/src/dotty/tools/debug/ResolveReflectEval.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala]
    Tuple=498 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala]
    Typed=7 files=[compiler/src/dotty/tools/dotc/cc/Synthetics.scala, compiler/src/dotty/tools/dotc/transform/Recheck.scala]
    other=0 files=[]
    wildcard=0 files=[]
  source_pattern_counts:
    1=749
    2=35
    3=5
    4=1
  binder_counts:
    0=1
    1=89
    2=573
    3+=127
  modifiers:
    lazy val=2
    val=751
    var=37
  explicit_tpt:
    explicit PatDef-wide tpt=5
    synthetic inferred TypeTree=785
  rhs:
    missing=0
    present=790
    recovery error=0
  typing_outcome_totals:
    failure::ApplicationCalleeNotMethod=1 files=1
    failure::ImportQualifierNotFound=86 files=49
    failure::LocalPatDefDeferred::multiple source patterns=23 files=13
    failure::MemberLookup=1 files=1
    failure::MissingDeclaredType=2 files=1
    failure::OverloadedTypeApplicationDeferred=1 files=1
    failure::SymbolResolution=3 files=3
    failure::TermNameNotFound=3 files=3
    failure::TypeNameNotFound=26 files=7
    failure::UnsupportedTypeTree=7 files=5
    not_attempted=637 files=171
  typing_attempt_statuses:
    profiled=790
    outside_typed_method_ranges_or_without_source_range=0
    success=0 files=0
    failure=153 files=79
    not_attempted=637 files=171
  typing_outcomes:
    failure::ApplicationCalleeNotMethod::val::binders=3+::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala]
    failure::ImportQualifierNotFound::val::binders=1::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=11 files=8 [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/core/TypeErasure.scala, compiler/src/dotty/tools/dotc/reporting/messages.scala, compiler/src/dotty/tools/dotc/transform/TreeChecker.scala, compiler/src/dotty/tools/dotc/transform/TupleOptimizations.scala]
    failure::ImportQualifierNotFound::val::binders=1::root=Bind::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/typer/Deriving.scala]
    failure::ImportQualifierNotFound::val::binders=1::root=InfixOp::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala]
    failure::ImportQualifierNotFound::val::binders=2::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=23 files=19 [compiler/src/dotty/tools/backend/jvm/PostProcessor.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/printing/RefinedPrinter.scala]
    failure::ImportQualifierNotFound::val::binders=2::root=InfixOp::tpt=synthetic inferred TypeTree=3 files=3 [compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/transform/TupleOptimizations.scala, compiler/src/dotty/tools/dotc/typer/Typer.scala]
    failure::ImportQualifierNotFound::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=29 files=24 [compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/Bench.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/classpath/ZipAndJarFileLookupFactory.scala]
    failure::ImportQualifierNotFound::val::binders=3+::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=8 files=8 [compiler/src/dotty/tools/dotc/ast/TreeTypeMap.scala, compiler/src/dotty/tools/dotc/printing/RefinedPrinter.scala, compiler/src/dotty/tools/dotc/reporting/messages.scala, compiler/src/dotty/tools/dotc/transform/Erasure.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala]
    failure::ImportQualifierNotFound::val::binders=3+::root=Bind::tpt=synthetic inferred TypeTree=6 files=5 [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/transform/Erasure.scala, compiler/src/dotty/tools/dotc/transform/SuperAccessors.scala, compiler/src/dotty/tools/dotc/transform/TreeChecker.scala, compiler/src/dotty/tools/dotc/typer/Dynamic.scala]
    failure::ImportQualifierNotFound::val::binders=3+::root=Tuple::tpt=synthetic inferred TypeTree=4 files=4 [compiler/src/dotty/tools/dotc/ast/DesugarEnums.scala, compiler/src/dotty/tools/dotc/quoted/QuotePatterns.scala, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala, compiler/src/scala/quoted/runtime/impl/QuoteMatcher.scala]
    failure::LocalPatDefDeferred::multiple source patterns::val::binders=2::root=Ident::tpt=synthetic inferred TypeTree=5 files=4 [compiler/src/dotty/tools/dotc/transform/FirstTransform.scala, library/src/scala/collection/ArrayOps.scala, library/src/scala/collection/StrictOptimizedIterableOps.scala, library/src/scala/collection/StringOps.scala]
    failure::LocalPatDefDeferred::multiple source patterns::var::binders=2::root=Ident::tpt=explicit PatDef-wide tpt=4 files=4 [library/src/scala/collection/immutable/LazyList.scala, library/src/scala/collection/immutable/LazyListIterable.scala, library/src/scala/collection/immutable/Set.scala, library/src/scala/collection/immutable/Stream.scala]
    failure::LocalPatDefDeferred::multiple source patterns::var::binders=2::root=Ident::tpt=synthetic inferred TypeTree=11 files=6 [library/src/scala/collection/ArrayOps.scala, library/src/scala/collection/StringOps.scala, library/src/scala/collection/immutable/Map.scala, library/src/scala/collection/mutable/AnyRefMap.scala, library/src/scala/collection/mutable/Buffer.scala]
    failure::LocalPatDefDeferred::multiple source patterns::var::binders=3+::root=Ident::tpt=explicit PatDef-wide tpt=1 files=1 [library/src/scala/collection/immutable/Set.scala]
    failure::LocalPatDefDeferred::multiple source patterns::var::binders=3+::root=Ident::tpt=synthetic inferred TypeTree=2 files=2 [library/src/scala/collection/immutable/Map.scala, library/src/scala/util/hashing/MurmurHash3.scala]
    failure::MemberLookup::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/util/Spans.scala]
    failure::MissingDeclaredType::val::binders=2::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=2 files=1 [compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala]
    failure::OverloadedTypeApplicationDeferred::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/util/Store.scala]
    failure::SymbolResolution::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=2 files=2 [library/src/scala/collection/immutable/LazyList.scala, library/src/scala/collection/mutable/ListMap.scala]
    failure::SymbolResolution::val::binders=3+::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/core/TyperState.scala]
    failure::TermNameNotFound::val::binders=1::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/classpath/AggregateClassPath.scala]
    failure::TermNameNotFound::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=2 files=2 [library/src/scala/collection/immutable/TreeMap.scala, library/src/scala/collection/immutable/TreeSet.scala]
    failure::TypeNameNotFound::val::binders=2::root=InfixOp::tpt=synthetic inferred TypeTree=2 files=2 [compiler/src/dotty/tools/debug/ExpressionCompiler.scala, library/src/scala/concurrent/duration/Duration.scala]
    failure::TypeNameNotFound::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=5 files=4 [compiler/src/dotty/tools/dotc/fromtasty/Debug.scala, compiler/src/dotty/tools/scripting/Main.scala, library/src/scala/collection/Iterator.scala, library/src/scala/collection/immutable/TreeSeqMap.scala]
    failure::TypeNameNotFound::val::binders=3+::root=Tuple::tpt=synthetic inferred TypeTree=19 files=2 [compiler/src/dotty/tools/scripting/Main.scala, library/src/scala/quoted/ToExpr.scala]
    failure::UnsupportedTypeTree::val::binders=1::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/util/CommentParsing.scala]
    failure::UnsupportedTypeTree::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=5 files=4 [compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/util/CommentParsing.scala, compiler/src/scala/quoted/runtime/impl/printers/SourceCode.scala, library/src/scala/collection/Iterator.scala]
    failure::UnsupportedTypeTree::val::binders=3+::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/core/Names.scala]
    not_attempted::lazy val::binders=1::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala]
    not_attempted::lazy val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/typer/Migrations.scala]
    not_attempted::val::binders=0::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/dotc/transform/Memoize.scala]
    not_attempted::val::binders=1::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=41 files=29 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/TreeInfo.scala]
    not_attempted::val::binders=1::root=Bind::tpt=synthetic inferred TypeTree=4 files=4 [compiler/src/dotty/tools/dotc/transform/ElimByName.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/transform/Recheck.scala, library/src/scala/quoted/Quotes.scala]
    not_attempted::val::binders=1::root=InfixOp::tpt=synthetic inferred TypeTree=6 files=6 [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/cc/Synthetics.scala, compiler/src/dotty/tools/dotc/transform/Bridges.scala, compiler/src/dotty/tools/dotc/transform/ContextFunctionResults.scala]
    not_attempted::val::binders=1::root=Tuple::tpt=synthetic inferred TypeTree=15 files=12 [compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala, compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/classpath/DirectoryClassPath.scala]
    not_attempted::val::binders=1::root=Typed::tpt=synthetic inferred TypeTree=7 files=2 [compiler/src/dotty/tools/dotc/cc/Synthetics.scala, compiler/src/dotty/tools/dotc/transform/Recheck.scala]
    not_attempted::val::binders=2::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=65 files=28 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala]
    not_attempted::val::binders=2::root=Bind::tpt=synthetic inferred TypeTree=2 files=2 [compiler/src/dotty/tools/dotc/core/tasty/TreePickler.scala, compiler/src/dotty/tools/dotc/transform/Constructors.scala]
    not_attempted::val::binders=2::root=Ident::tpt=synthetic inferred TypeTree=4 files=4 [compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/Constructors.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala, compiler/src/dotty/tools/dotc/typer/Typer.scala]
    not_attempted::val::binders=2::root=InfixOp::tpt=synthetic inferred TypeTree=28 files=22 [compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/debug/ResolveReflectEval.scala, compiler/src/dotty/tools/dotc/config/Settings.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala]
    not_attempted::val::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=361 files=127 [compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala]
    not_attempted::val::binders=3+::root=Apply / extractor-looking::tpt=synthetic inferred TypeTree=25 files=12 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/config/PathResolver.scala, compiler/src/dotty/tools/dotc/core/SymDenotations.scala]
    not_attempted::val::binders=3+::root=Bind::tpt=synthetic inferred TypeTree=11 files=8 [compiler/src/dotty/tools/dotc/ast/TreeMapWithTrackedStats.scala, compiler/src/dotty/tools/dotc/cc/Setup.scala, compiler/src/dotty/tools/dotc/core/ConstraintHandling.scala, compiler/src/dotty/tools/dotc/core/Denotations.scala, compiler/src/dotty/tools/dotc/core/classfile/ClassfileParser.scala]
    not_attempted::val::binders=3+::root=Ident::tpt=synthetic inferred TypeTree=1 files=1 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala]
    not_attempted::val::binders=3+::root=InfixOp::tpt=synthetic inferred TypeTree=2 files=2 [compiler/src/dotty/tools/dotc/core/OrderingConstraint.scala, compiler/src/dotty/tools/dotc/transform/ElimRepeated.scala]
    not_attempted::val::binders=3+::root=Tuple::tpt=synthetic inferred TypeTree=43 files=29 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala, compiler/src/dotty/tools/backend/jvm/PostProcessor.scala, compiler/src/dotty/tools/backend/jvm/opt/Inliner.scala, compiler/src/dotty/tools/backend/jvm/opt/LocalOpt.scala]
    not_attempted::var::binders=2::root=Ident::tpt=synthetic inferred TypeTree=11 files=5 [library/src/scala/collection/Seq.scala, library/src/scala/collection/immutable/Map.scala, library/src/scala/collection/mutable/AnyRefMap.scala, library/src/scala/collection/mutable/ArrayDeque.scala, library/src/scala/collection/mutable/LongMap.scala]
    not_attempted::var::binders=2::root=Tuple::tpt=synthetic inferred TypeTree=6 files=4 [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/dotc/parsing/JavaParsers.scala, compiler/src/dotty/tools/dotc/plugins/Plugins.scala, compiler/src/dotty/tools/dotc/typer/Typer.scala]
    not_attempted::var::binders=3+::root=Ident::tpt=synthetic inferred TypeTree=2 files=2 [library/src/scala/collection/immutable/Map.scala, library/src/scala/collection/immutable/RedBlackTree.scala]
  representative_files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala]
unsupported_expression_total=25
expression_sprint_first_blockers:
  UnsupportedExpression::PrefixOp=0 files=0
  UnsupportedExpression::Annotated=0 files=0
  MemberNotFound=1 files=1
  MemberLookup=61 files=28
  TypeNameNotFound=80 files=30
  SourceAnnotationClassDeferred=0 files=0
  SourceAnnotationNotAnnotationClass=0 files=0
  SourceAnnotationArgumentNotConstant=0 files=0
  SourceAnnotationConstructorDeferred=0 files=0
  SourceAnnotationConstructorArgumentMismatch=0 files=0
  SourceAnnotationArgumentTypeDeferred=0 files=0
  ExpressionTypeCannotBeWidened=0 files=0
  ExpectedExpressionTypeMismatch=0 files=0
  ExpectedExpressionConformanceUnsupported=0 files=0
UnsupportedTypeTree=20 files=6
unsupported_type_tree_failures:
  UnsupportedTypeTree::FunctionWithMods: count=13, files=2 [library/src/scala/collection/StringParsers.scala, library/src/scala/collection/convert/JavaCollectionWrappers.scala]
  UnsupportedTypeTree::Annotated: count=5, files=3 [library/src/scala/collection/Iterator.scala, library/src/scala/collection/immutable/ArraySeq.scala, library/src/scala/collection/immutable/LazyListIterable.scala]
  UnsupportedTypeTree::Tuple: count=2, files=1 [compiler/src/scala/quoted/runtime/impl/printers/SourceCode.scala]
local_block_declaration_deferred=41
no_successful_enclosing_method_typing=34
import_qualifier_not_found=2876
external_name_or_member_resolution_failures=202
failure_families:
  resolution/classpath environment=3079
  other=141
  local declaration deferral=46
  unsupported expression syntax/semantics=25
  type relation/inference/completion=6
local_defdef_failures:
  ImportQualifierNotFound [resolution/classpath environment]: 2876 (273 files) [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala]
  TypeNameNotFound [resolution/classpath environment]: 80 (30 files) [compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/dotc/core/tasty/CommentPickler.scala, compiler/src/dotty/tools/dotc/core/tasty/TreeBuffer.scala, compiler/src/dotty/tools/dotc/coverage/Serializer.scala, compiler/src/dotty/tools/dotc/util/Chars.scala]
  MemberLookup [resolution/classpath environment]: 61 (28 files) [compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Denotations.scala, compiler/src/dotty/tools/dotc/core/SymDenotations.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/core/classfile/ClassfileParser.scala]
  SymbolResolution [resolution/classpath environment]: 40 (17 files) [compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/core/Decorators.scala, compiler/src/dotty/tools/dotc/core/Denotations.scala, compiler/src/dotty/tools/dotc/core/SymbolLoaders.scala]
  NoSuccessfulEnclosingMethodTyping [other]: 34 (15 files) [compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/classpath/DirectoryClassPath.scala, compiler/src/dotty/tools/dotc/classpath/ZipAndJarFileLookupFactory.scala, compiler/src/dotty/tools/dotc/core/Definitions.scala]
  AnonymousClassInstantiationDeferred [other]: 32 (15 files) [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/cc/Setup.scala, compiler/src/dotty/tools/dotc/core/Definitions.scala]
  LocalBlockDeclarationDeferred::val/var definition [local declaration deferral]: 21 (8 files) [compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala]
  TermNameNotFound [resolution/classpath environment]: 20 (12 files) [compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/opt/MethodMax.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala, compiler/src/dotty/tools/dotc/util/ClasspathFromClassloader.scala, compiler/src/dotty/tools/dotc/util/WeakHashSet.scala]
  UnsupportedTypeTree::FunctionWithMods [other]: 13 (2 files) [library/src/scala/collection/StringParsers.scala, library/src/scala/collection/convert/JavaCollectionWrappers.scala]
  LocalBlockDeclarationDeferred::type definition [local declaration deferral]: 11 (4 files) [compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/typer/Checking.scala, compiler/src/dotty/tools/dotc/typer/Typer.scala]
  SymbolSourceKindMismatch [other]: 11 (3 files) [compiler/src/dotty/tools/dotc/parsing/Tokens.scala, compiler/src/dotty/tools/dotc/typer/Namer.scala, library/src/scala/collection/immutable/Vector.scala]
  UnsupportedExpression::ParsedTry [unsupported expression syntax/semantics]: 11 (6 files) [compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala, compiler/src/dotty/tools/dotc/transform/Erasure.scala, compiler/src/dotty/tools/dotc/transform/ExplicitOuter.scala]
  MissingDeclaredType [other]: 10 (3 files) [compiler/src/dotty/tools/dotc/core/NamerOps.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala]
  LocalBlockDeclarationDeferred::module definition [local declaration deferral]: 9 (2 files) [compiler/src/dotty/tools/dotc/ast/DesugarEnums.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala]
  TypedPatternRuntimeTestDeferred [other]: 9 (2 files) [compiler/src/dotty/tools/dotc/core/Types.scala, library/src/scala/collection/immutable/HashMap.scala]
  UnsupportedFunctionLiteralParameter [other]: 9 (1 files) [compiler/src/dotty/tools/dotc/typer/Synthesizer.scala]
  StringLiteralTypingDeferred [other]: 8 (5 files) [compiler/src/dotty/tools/dotc/core/NameOps.scala, compiler/src/dotty/tools/dotc/core/TypeErrors.scala, compiler/src/dotty/tools/dotc/core/tasty/TastyPrinter.scala, compiler/src/dotty/tools/dotc/reporting/messages.scala, library/src/scala/util/Random.scala]
  UnsupportedExpression::ForDo [unsupported expression syntax/semantics]: 5 (2 files) [compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/transform/LambdaLift.scala]
  UnsupportedExpression::InterpolatedString [unsupported expression syntax/semantics]: 5 (4 files) [compiler/src/dotty/tools/dotc/core/ConstraintHandling.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/report.scala, compiler/src/dotty/tools/dotc/util/SimpleIdentityMap.scala]
  UnsupportedTypeTree::Annotated [other]: 5 (3 files) [library/src/scala/collection/Iterator.scala, library/src/scala/collection/immutable/ArraySeq.scala, library/src/scala/collection/immutable/LazyListIterable.scala]
  MatchSelectorTypeCannotBeAdapted [other]: 4 (2 files) [compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/config/Settings.scala]
  UnstableSelectionPrefix [type relation/inference/completion]: 4 (3 files) [compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala, compiler/src/dotty/tools/dotc/core/Types.scala]
  LocalExtensionGroupShapeDeferred [local declaration deferral]: 3 (1 files) [compiler/src/dotty/tools/dotc/typer/Applications.scala]
  LocalPatDefDeferred::multiple source patterns [local declaration deferral]: 2 (1 files) [compiler/src/dotty/tools/dotc/transform/FirstTransform.scala]
  RightAssociativeInfixDeferred [other]: 2 (2 files) [compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/typer/ImportInfo.scala]
  UnsupportedExpression::ForYield [unsupported expression syntax/semantics]: 2 (1 files) [compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala]
  UnsupportedExpression::Throw [unsupported expression syntax/semantics]: 2 (2 files) [compiler/src/dotty/tools/backend/jvm/BTypes.scala, library/src/scala/collection/immutable/Queue.scala]
  UnsupportedTypeTree::Tuple [other]: 2 (1 files) [compiler/src/scala/quoted/runtime/impl/printers/SourceCode.scala]
  ApplicationCalleeNotMethod [type relation/inference/completion]: 1 (1 files) [compiler/src/dotty/tools/dotc/core/TypeErasure.scala]
  ExtractorQualifierNotValueLike [other]: 1 (1 files) [compiler/src/scala/quoted/runtime/impl/QuoteMatcher.scala]
  LocalExtensionReceiverTypeNotFound [resolution/classpath environment]: 1 (1 files) [compiler/src/dotty/tools/dotc/core/Flags.scala]
  LocalExtensionSignatureDeferred::dependent result types [type relation/inference/completion]: 1 (1 files) [compiler/src/dotty/tools/dotc/reporting/MessageRendering.scala]
  MemberNotFound [resolution/classpath environment]: 1 (1 files) [compiler/src/dotty/tools/backend/jvm/BTypes.scala]
  PatternTypeRelationDeferred [other]: 1 (1 files) [library/src/scala/collection/immutable/List.scala]
source_function_method_outcomes:
  expression::Function::ImportQualifierNotFound: count=4, files=1, examples=[compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:737:refersTo, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:752:removeSingleton, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:754:mapArg, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:758:elim]
  expression::Function::UnsupportedFunctionLiteralParameter: count=9, files=1, examples=[compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:710:factoryManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:718:singletonManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:721:synthArrayManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:727:synthWildcardManifest, compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:731:synthArgManifests]
  type::Function::SymbolResolution: count=12, files=8, examples=[compiler/src/dotty/tools/backend/jvm/BTypes.scala:671:ifInit, compiler/src/dotty/tools/backend/jvm/BTypes.scala:673:isJLO, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2387:genArgs, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2388:genArgsAsClassCaptures, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:3386:genScalaArgs]
  method=compiler/src/dotty/tools/backend/jvm/BTypes.scala:671:ifInit baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/backend/jvm/BTypes.scala:673:isJLO baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2387:genArgs baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:2388:genArgsAsClassCaptures baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:3386:genScalaArgs baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala:3387:genJSArgs baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/dotc/core/Denotations.scala:310:argStr baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/dotc/core/Types.scala:3448:normalize baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:737:refersTo baseline_form=expression::Function first_blocker=ImportQualifierNotFound
  method=compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:752:removeSingleton baseline_form=expression::Function first_blocker=ImportQualifierNotFound
  method=compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:754:mapArg baseline_form=expression::Function first_blocker=ImportQualifierNotFound
  method=compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala:758:elim baseline_form=expression::Function first_blocker=ImportQualifierNotFound
  method=compiler/src/dotty/tools/dotc/parsing/Parsers.scala:3395:maybeAscription baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/dotc/transform/PostTyper.scala:366:unusable baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/dotc/typer/ProtoTypes.scala:409:isPoly baseline_form=type::Function first_blocker=SymbolResolution
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:710:factoryManifest baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:718:singletonManifest baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:721:synthArrayManifest baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:727:synthWildcardManifest baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:731:synthArgManifests baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:741:canManifest baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:748:synthManifest baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:770:manifestOfType baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala:774:synthesize baseline_form=expression::Function first_blocker=UnsupportedFunctionLiteralParameter
  method=library/src/scala/util/control/Exception.scala:416:fun baseline_form=type::Function first_blocker=SymbolResolution
missing_declared_type_profile:
  first_blockers=10 distinct_declarations=3
  value::Field::owner=ModuleClass::synthetic inferred TypeTree::rhs=true::modifiers=::semantic_mutable=false: 6 (2 files) [compiler/src/dotty/tools/dotc/core/NamerOps.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala]
  value::Field::owner=ModuleClass::synthetic inferred TypeTree::rhs=true::modifiers=Inline::semantic_mutable=false: 4 (1 files) [compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala]
missing_declared_type_records:
  compiler/src/dotty/tools/dotc/core/NamerOps.scala: failed_local_method_tree=929 tree=607 error_tree_kind=TypeTree declaration_tree=611 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 6734, end: 6734 }, point: None } })
  compiler/src/dotty/tools/dotc/core/NamerOps.scala: failed_local_method_tree=940 tree=607 error_tree_kind=TypeTree declaration_tree=611 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 6734, end: 6734 }, point: None } })
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala: failed_local_method_tree=1773 tree=6233 error_tree_kind=TypeTree declaration_tree=6238 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 63943, end: 63943 }, point: None } })
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala: failed_local_method_tree=1797 tree=6233 error_tree_kind=TypeTree declaration_tree=6238 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 63943, end: 63943 }, point: None } })
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala: failed_local_method_tree=1898 tree=6233 error_tree_kind=TypeTree declaration_tree=6238 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 63943, end: 63943 }, point: None } })
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala: failed_local_method_tree=1917 tree=6233 error_tree_kind=TypeTree declaration_tree=6238 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 63943, end: 63943 }, point: None } })
  compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala: failed_local_method_tree=3861 tree=175 error_tree_kind=TypeTree declaration_tree=177 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[Inline] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 2358, end: 2358 }, point: None } })
  compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala: failed_local_method_tree=3906 tree=175 error_tree_kind=TypeTree declaration_tree=177 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[Inline] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 2358, end: 2358 }, point: None } })
  compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala: failed_local_method_tree=3987 tree=175 error_tree_kind=TypeTree declaration_tree=177 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[Inline] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 2358, end: 2358 }, point: None } })
  compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala: failed_local_method_tree=4009 tree=175 error_tree_kind=TypeTree declaration_tree=177 declaration_tree_kind=ValDef declaration=value symbol_kind=Field owner_kind=ModuleClass context_owner_kind=ModuleClass shape=synthetic inferred TypeTree rhs=true modifiers=[Inline] semantic_mutable=false entry=complete_symbol_inner -> type_of_tpt_inner_journaled span=Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 2358, end: 2358 }, point: None } })
top_semantic_gaps:
  1. AnonymousClassInstantiationDeferred: count=32, files=15, category=other, examples=compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/cc/Setup.scala, compiler/src/dotty/tools/dotc/core/Definitions.scala
  2. LocalBlockDeclarationDeferred::val/var definition: count=21, files=8, category=local declaration support, examples=compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala
  3. UnsupportedTypeTree::FunctionWithMods: count=13, files=2, category=other, examples=library/src/scala/collection/StringParsers.scala, library/src/scala/collection/convert/JavaCollectionWrappers.scala
  4. LocalBlockDeclarationDeferred::type definition: count=11, files=4, category=local declaration support, examples=compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/typer/Checking.scala, compiler/src/dotty/tools/dotc/typer/Typer.scala
  5. SymbolSourceKindMismatch: count=11, files=3, category=other, examples=compiler/src/dotty/tools/dotc/parsing/Tokens.scala, compiler/src/dotty/tools/dotc/typer/Namer.scala, library/src/scala/collection/immutable/Vector.scala
  6. UnsupportedExpression::ParsedTry: count=11, files=6, category=expression typing, examples=compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala, compiler/src/dotty/tools/dotc/transform/Erasure.scala, compiler/src/dotty/tools/dotc/transform/ExplicitOuter.scala
  7. MissingDeclaredType: count=10, files=3, category=other, examples=compiler/src/dotty/tools/dotc/core/NamerOps.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala
  8. LocalBlockDeclarationDeferred::module definition: count=9, files=2, category=local declaration support, examples=compiler/src/dotty/tools/dotc/ast/DesugarEnums.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala
  9. TypedPatternRuntimeTestDeferred: count=9, files=2, category=pattern typing, examples=compiler/src/dotty/tools/dotc/core/Types.scala, library/src/scala/collection/immutable/HashMap.scala
  10. UnsupportedFunctionLiteralParameter: count=9, files=1, category=other, examples=compiler/src/dotty/tools/dotc/typer/Synthesizer.scala
top_gap_implementation_scope_notes:
  AnonymousClassInstantiationDeferred (32 occurrences, 15 files): first_slice=support one anonymous new with one concrete parent and explicit member ownership; owner=dotty-typer/src/typer/expression/new.rs; prerequisites=ordinary New typing, parent projection, and stable anonymous class identity; non_goals=closure capture, refinement synthesis, and general anonymous-class members
  LocalBlockDeclarationDeferred::val/var definition (21 occurrences, 8 files): first_slice=split remaining local definitions by PatDef root and binder shape before adding one form; owner=dotty-typer/src/typer/expression/blocks.rs; prerequisites=transactional PatDef lowering, local binders, and assignment support; non_goals=general destructuring or reopening already supported PatDef forms
  UnsupportedTypeTree::FunctionWithMods (13 occurrences, 2 files): first_slice=inspect the remaining modifier-bearing function types and keep erased/capture-specific forms deferred; owner=dotty-typer/src/typer/type_projection.rs; prerequisites=plain contextual `Given` forms now use the canonical ContextFunction identity and existing Applied types; non_goals=capture checking, erased-function semantics, and arbitrary modifiers
  LocalBlockDeclarationDeferred::type definition (11 occurrences, 4 files): first_slice=enter one local declaration kind transactionally in block typing; owner=dotty-typer/src/typer/expression/blocks.rs; prerequisites=source symbol and scope metadata from dotty-core; non_goals=local classes, imports, or type definitions beyond the selected kind
  SymbolSourceKindMismatch (11 occurrences, 3 files): first_slice=reproduce the exact error bucket with a focused semantic fixture; owner=the narrow module producing that TyperError; prerequisites=the relevant source semantic metadata; non_goals=adjacent unsupported language features
  UnsupportedExpression::ParsedTry (11 occurrences, 6 files): first_slice=lower one reported expression node through existing expression typing; owner=dotty-typer/src/typer/expression; prerequisites=the parsed AST node and its child typing rules; non_goals=control-flow or inference redesign
  MissingDeclaredType (10 occurrences, 3 files): first_slice=infer one ordinary or inline inferred module-class field after source class val/var inference; the remaining bucket is 10 occurrences across 3 files; owner=dotty-typer/src/typer/completion/mod.rs, completion/declarations.rs, type_projection.rs, and existing expression typing; prerequisites=module initialization context, RHS typing and widening, cycle behavior, and completion rollback; non_goals=class fields, method results, local PatDef, and generalized expected-type inference
  LocalBlockDeclarationDeferred::module definition (9 occurrences, 2 files): first_slice=enter one local declaration kind transactionally in block typing; owner=dotty-typer/src/typer/expression/blocks.rs; prerequisites=source symbol and scope metadata from dotty-core; non_goals=local classes, imports, or type definitions beyond the selected kind
  TypedPatternRuntimeTestDeferred (9 occurrences, 2 files): first_slice=type one pattern form against an already known expected type; owner=dotty-typer/src/typer/patterns; prerequisites=the expected type and existing pattern AST shape; non_goals=exhaustivity analysis and match-result inference
  UnsupportedFunctionLiteralParameter (9 occurrences, 1 files): first_slice=reproduce the exact error bucket with a focused semantic fixture; owner=the narrow module producing that TyperError; prerequisites=the relevant source semantic metadata; non_goals=adjacent unsupported language features
highest_ranked_semantic_gap: AnonymousClassInstantiationDeferred (32 occurrences in 15 files); count ranks the audit only and does not select a sprint increment; keep classpath materialization as a separate gate because the pinned audit resolved no external members
match_readiness:
  first_blocker_methods=279
  structural_matches_in_first_blocker_methods=310
  cases_in_first_blocker_methods=858
  guarded_cases=72
  unguarded_cases=786
  first_blocker_errors:
    ExtractorPatternConstraintDeferred=1 files=[library/src/scala/quoted/Quotes.scala]
    ExtractorQualifierMemberNotFound=26 files=[library/src/scala/collection/immutable/IntMap.scala, library/src/scala/collection/immutable/LongMap.scala]
    ExtractorQualifierNotFound=4 files=[compiler/src/dotty/tools/dotc/config/ScalaVersion.scala]
    ExtractorQualifierNotValueLike=5 files=[compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/scala/quoted/runtime/impl/QuoteMatcher.scala, library/src/scala/collection/immutable/IntMap.scala]
    ExtractorUnapplyNotFound=7 files=[compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Types.scala]
    MatchSelectorTypeCannotBeAdapted=18 files=[compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/config/Settings.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala]
    stable identifier: PatternTypeRelationDeferred=9 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala, library/src/scala/collection/immutable/List.scala]
    stable selection: PatternTypeRelationDeferred=10 files=[library/src/scala/collection/immutable/IntMap.scala, library/src/scala/collection/immutable/LongMap.scala]
    typed pattern: TypedPatternRelationDeferred=31 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, library/src/scala/collection/immutable/ArraySeq.scala, library/src/scala/collection/immutable/Range.scala]
    typed pattern: TypedPatternRuntimeTestDeferred=167 files=[compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/core/Annotations.scala]
    unknown: PatternTypeRelationDeferred=1 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala]
  pattern_root_shapes:
    alternative=11 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/transform/init/Semantic.scala]
    extractor-looking Apply/TypeApply=119 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala]
    literal=9 files=[library/src/scala/collection/immutable/Map.scala]
    other=45 files=[compiler/src/dotty/tools/dotc/config/Settings.scala, compiler/src/dotty/tools/dotc/core/Types.scala, library/src/scala/collection/immutable/IntMap.scala, library/src/scala/collection/immutable/List.scala, library/src/scala/collection/immutable/LongMap.scala]
    typed pattern=378 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CaptureSet.scala]
    wildcard/identifier/bind=296 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala]
match_corpus_profile:
  matches=7444
  cases=21835
  guarded_cases=2118
  unguarded_cases=19717
  pattern_root_shapes:
    alternative=443 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala]
    extractor-looking Apply=4314 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala]
    infix pattern=469 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/opt/InlinerHeuristics.scala]
    literal=1365 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/jvm/CodeGen.scala]
    other=108 files=[compiler/src/dotty/tools/debug/ResolveReflectEval.scala, compiler/src/dotty/tools/dotc/ast/NavigateAST.scala, compiler/src/dotty/tools/dotc/semanticdb/ExtractSemanticDB.scala, compiler/src/dotty/tools/dotc/staging/TreeMapWithStages.scala, compiler/src/dotty/tools/dotc/transform/localopt/StringInterpolatorOpt.scala]
    stable identifier=1745 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala]
    stable selection=625 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala]
    tuple=479 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala]
    typed explicit Bind=3 files=[compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/io/FileWriters.scala]
    typed variable=5401 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala]
    typed wildcard=382 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/GeneratedClassHandler.scala, compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala, compiler/src/dotty/tools/backend/jvm/analysis/AliasingAnalyzer.scala, compiler/src/dotty/tools/backend/jvm/analysis/ProdConsAnalyzer.scala]
    variable identifier=1146 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BTypeLoader.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala]
    wildcard=4675 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/ScalaPrimitivesOps.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala]
    wildcard/identifier/bind=680 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/GenericSignatures.scala, compiler/src/dotty/tools/backend/jvm/opt/BCodeRepository.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala]
  typed_case_successes:
    alternative=0 files=[]
    extractor-looking Apply=0 files=[]
    extractor-looking TypeApply=0 files=[]
    guarded supported case=0 files=[]
    infix pattern=0 files=[]
    literal=0 files=[]
    stable identifier/selection=0 files=[]
    tuple=0 files=[]
    typed explicit Bind=0 files=[]
    typed variable=0 files=[]
    typed wildcard=0 files=[]
    variable/bind=33 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala]
    wildcard=207 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala]
  successful_extractor_protocols:
  typed_pattern_boundaries:
    generic/erased or unsupported runtime test=474
    unsupported type-tree projection=108
    unsupported typed-pattern relation=43
  typed_case_first_failures:
    UnsupportedTypeTree=243
    AmbiguousTermReference=6 files=[compiler/src/dotty/tools/dotc/parsing/Parsers.scala]
    ApplicationCalleeNotMethod=7 files=[compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/UnrollDefinitions.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala]
    ExtractorPatternConstraintDeferred=3 files=[library/src/scala/quoted/Quotes.scala]
    ExtractorQualifierMemberNotFound=63 files=[compiler/src/dotty/tools/dotc/quoted/PickledQuotes.scala, library/src/scala/collection/immutable/IntMap.scala, library/src/scala/collection/immutable/LongMap.scala]
    ExtractorQualifierNotFound=42 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/classpath/AggregateClassPath.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala, compiler/src/dotty/tools/dotc/util/DiffUtil.scala, compiler/src/dotty/tools/scripting/Main.scala]
    ExtractorQualifierNotValueLike=18 files=[compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/quoted/PickledQuotes.scala, compiler/src/dotty/tools/dotc/typer/Namer.scala]
    ExtractorUnapplyNotFound=52 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/classpath/AggregateClassPath.scala, compiler/src/dotty/tools/dotc/config/Settings.scala, compiler/src/dotty/tools/dotc/core/Types.scala]
    ImportQualifierNotFound=14509 files=[compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala]
    InfixPatternDeferred=7 files=[compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/printing/PlainPrinter.scala, compiler/src/dotty/tools/dotc/typer/Applications.scala, compiler/src/dotty/tools/dotc/typer/Deriving.scala]
    MatchCaseResultTypeCannotBeWidened=1 files=[compiler/src/dotty/tools/dotc/core/Types.scala]
    MatchSelectorTypeCannotBeAdapted=54 files=[compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/config/Settings.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala]
    MemberLookup=257 files=[compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala, compiler/src/dotty/tools/debug/ResolveReflectEval.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala]
    MemberNotFound=51 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/cc/Mutability.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/transform/init/Objects.scala, compiler/src/dotty/tools/dotc/transform/init/Semantic.scala]
    MissingDeclaredType=11 files=[compiler/src/dotty/tools/backend/jvm/opt/BCodeRepository.scala, compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala, library/src/scala/collection/convert/JavaCollectionWrappers.scala]
    NullLiteralTypingDeferred=7 files=[compiler/src/dotty/tools/dotc/core/Types.scala, library/src/scala/collection/immutable/RedBlackTree.scala, library/src/scala/collection/mutable/CollisionProofHashMap.scala, library/src/scala/collection/mutable/RedBlackTree.scala, library/src/scala/runtime/ScalaRunTime.scala]
    ObjectTermReferenceDeferred=7 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/semanticdb/PPrint.scala]
    OverloadedReferenceDeferred=51 files=[compiler/src/dotty/tools/dotc/core/Types.scala]
    PatternTypeMismatch=12 files=[compiler/src/dotty/tools/dotc/reporting/Message.scala, compiler/src/dotty/tools/dotc/transform/localopt/FormatChecker.scala, compiler/src/dotty/tools/dotc/util/Signatures.scala]
    PatternTypeRelationDeferred=111 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala, compiler/src/dotty/tools/dotc/core/Types.scala]
    RightAssociativeInfixDeferred=3 files=[compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/transform/Mixin.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala]
    StringLiteralTypingDeferred=13 files=[compiler/src/dotty/tools/dotc/config/PathResolver.scala, compiler/src/dotty/tools/dotc/core/Denotations.scala, compiler/src/dotty/tools/dotc/core/Flags.scala, compiler/src/dotty/tools/dotc/transform/localopt/FormatChecker.scala, compiler/src/dotty/tools/scripting/Main.scala]
    SymbolResolution=606 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/analysis/AliasingAnalyzer.scala]
    TermNameNotFound=313 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/opt/MethodMax.scala, compiler/src/dotty/tools/dotc/classpath/AggregateClassPath.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala]
    TypeNameNotFound=770 files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/classpath/AggregateClassPath.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala]
    TypedPatternRelationDeferred=44 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala, compiler/src/dotty/tools/dotc/transform/patmat/Space.scala, library/src/scala/collection/immutable/ArraySeq.scala, library/src/scala/collection/immutable/Range.scala]
    TypedPatternRuntimeTestDeferred=484 files=[compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/core/Annotations.scala]
    UnstableSelectionPrefix=2 files=[compiler/src/dotty/tools/backend/jvm/BTypes.scala]
    UnsupportedBindPatternBody=9 files=[compiler/src/dotty/tools/backend/jvm/opt/Inliner.scala, compiler/src/dotty/tools/dotc/inlines/Inliner.scala, compiler/src/dotty/tools/dotc/parsing/xml/MarkupParsers.scala, compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/dotc/typer/Applications.scala]
    UnsupportedConstructorInferenceShape=4 files=[library/src/scala/collection/immutable/IntMap.scala, library/src/scala/collection/immutable/LongMap.scala]
    UnsupportedExpression=370 files=[compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/opt/BTypesFromClassfile.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala]
    UnsupportedTermReference=1 files=[compiler/src/dotty/tools/dotc/cc/CaptureSet.scala]
    UnsupportedTypeTree::Annotated=150 files=[compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/core/Symbols.scala, compiler/src/dotty/tools/dotc/transform/Erasure.scala, library/src/scala/collection/ArrayOps.scala, library/src/scala/collection/Iterable.scala]
    UnsupportedTypeTree::FunctionWithMods=31 files=[library/src/scala/collection/ArrayOps.scala, library/src/scala/collection/IterableOnce.scala, library/src/scala/collection/StrictOptimizedIterableOps.scala, library/src/scala/collection/StringOps.scala, library/src/scala/collection/convert/JavaCollectionWrappers.scala]
    UnsupportedTypeTree::Tuple=62 files=[compiler/src/dotty/tools/backend/sjs/JSEncoding.scala, library/src/scala/collection/immutable/IntMap.scala, library/src/scala/collection/immutable/LongMap.scala, library/src/scala/collection/immutable/VectorMap.scala, library/src/scala/quoted/Quotes.scala]
  typed_case_first_failure_top_10:
    ImportQualifierNotFound=14509
    TypeNameNotFound=770
    SymbolResolution=606
    TypedPatternRuntimeTestDeferred=484
    UnsupportedExpression=370
    TermNameNotFound=313
    MemberLookup=257
    UnsupportedTypeTree::Annotated=150
    PatternTypeRelationDeferred=111
    ExtractorQualifierMemberNotFound=63
  extractor_root_shapes:
    Apply root=6010
  extractor_dispatch:
    selected extractor=439
    simple extractor identifier=5571
  extractor_type_applied=0
  extractor_argument_counts:
    0=45
    1=2419
    2=2845
    3=475
    4=159
    5=39
    6=5
    7=5
    8=3
    9=1
    10=2
    11=1
    12=1
    13=1
    14=1
    15=1
    16=1
    17=1
    18=1
    19=1
    20=1
    21=1
    22=1
  extractor_nested_argument_roots:
    alternative=6
    extractor-looking Apply=498
    infix pattern=172
    literal=73
    other=447
    stable identifier=183
    stable selection=163
    tuple=62
    typed variable=357
    typed wildcard=38
    variable identifier=6421
    wildcard=2157
    wildcard/identifier/bind=104
  sequence_wildcard_occurrences=0
  named_pattern_arguments=0
  empty_tuple_unit_patterns=8
  infix_pattern_forms=781
  extractor_representative_files=[compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala]
resolver_metrics:
  resolver_package_requests=22962
  external_package_requests=14150
  external_package_successes=3531
  external_package_unresolved=10619
  external_package_errors=0
  source_package_reuse=8812
  resolver_member_requests=7502
  external_member_requests=7502
  external_member_successes=0
  external_class_symbol_successes=0
  external_non_class_member_successes=0
  source_member_reuse=0
  external_member_unresolved=7159
  external_member_errors=342
  distinct_packages=25
  distinct_classes=0
  distinct_members=0
  classloader_success_gate=BLOCKED: external members not materialized
resolver_581_comparison:
  external_package_successes=3531 (baseline=1965, delta=+1566)
  external_package_unresolved=10619 (baseline=5816, delta=+4803)
  external_package_errors=0 (baseline=0, delta=+0)
  external_class_materializations=0 (baseline=0, delta=+0)
  external_non_class_member_successes=0 (baseline=0, delta=+0)
  external_member_unresolved=7159 (baseline=3884, delta=+3275)
  external_member_errors=342 (baseline=91, delta=+251)
  distinct_packages=25 (baseline=23, delta=+2)
  member_error_kinds:
    Malformed { reason: "class dotty/tools/dotc/CompilationUnit failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class dotty/tools/io/AbstractFile failed to load because java/lang/Object failed: class not found: java/lang/Object" }=13
    Malformed { reason: "class dotty/tools/io/ClassPath failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class dotty/tools/io/ClassRepresentation failed to load because java/lang/Object failed: class not found: java/lang/Object" }=7
    Malformed { reason: "class dotty/tools/tasty/TastyBuffer failed to load because java/lang/Object failed: class not found: java/lang/Object" }=3
    Malformed { reason: "class dotty/tools/tasty/TastyReader failed to load because java/lang/Object failed: class not found: java/lang/Object" }=8
    Malformed { reason: "class dotty/tools/tasty/TastyVersion failed to load because java/lang/Object failed: class not found: java/lang/Object" }=4
    Malformed { reason: "class scala/Function0 failed to load because java/lang/Object failed: class not found: java/lang/Object" }=13
    Malformed { reason: "class scala/Function2 failed to load because java/lang/Object failed: class not found: java/lang/Object" }=21
    Malformed { reason: "class scala/collection/AbstractIterator failed to load because java/lang/Object failed: class not found: java/lang/Object" }=3
    Malformed { reason: "class scala/collection/IndexedSeq failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class scala/collection/Iterator failed to load because java/lang/Object failed: class not found: java/lang/Object" }=2
    Malformed { reason: "class scala/collection/LinearSeq failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class scala/collection/SpecificIterableFactory failed to load because java/lang/Object failed: class not found: java/lang/Object" }=3
    Malformed { reason: "class scala/collection/StepperShape failed to load because java/lang/Object failed: class not found: java/lang/Object" }=3
    Malformed { reason: "class scala/collection/convert/impl/CodePointStringStepper failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class scala/collection/mutable/Builder failed to load because java/lang/Object failed: class not found: java/lang/Object" }=8
    Malformed { reason: "class scala/collection/mutable/Map failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class scala/collection/mutable/ReusableBuilder failed to load because java/lang/Object failed: class not found: java/lang/Object" }=21
    Malformed { reason: "class scala/io/Codec failed to load because java/lang/Object failed: class not found: java/lang/Object" }=2
    Malformed { reason: "class scala/math/ScalaNumber failed to load because java/lang/Number failed: class not found: java/lang/Number" }=1
    Malformed { reason: "class scala/quoted/Quotes failed to load because java/lang/Object failed: class not found: java/lang/Object" }=1
    Malformed { reason: "class scala/runtime/AbstractFunction2 failed to load because java/lang/Object failed: class not found: java/lang/Object" }=3
    Malformed { reason: "invalid .tasty file for dotty/tools/io/ManifestResources: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/Function1: a supertype reference could not be resolved to a name" }=101
    Malformed { reason: "invalid .tasty file for scala/Tuple2: a supertype reference could not be resolved to a name" }=2
    Malformed { reason: "invalid .tasty file for scala/Tuple3: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/annotation/switch: a supertype reference could not be resolved to a name" }=19
    Malformed { reason: "invalid .tasty file for scala/collection/IterableOnce: a supertype reference could not be resolved to a name" }=5
    Malformed { reason: "invalid .tasty file for scala/collection/Map: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/Seq: a supertype reference could not be resolved to a name" }=9
    Malformed { reason: "invalid .tasty file for scala/collection/Set: a supertype reference could not be resolved to a name" }=2
    Malformed { reason: "invalid .tasty file for scala/collection/SortedMapOps: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/WithFilter: a supertype reference could not be resolved to a name" }=7
    Malformed { reason: "invalid .tasty file for scala/collection/convert/impl/BoxedBooleanArrayStepper: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/convert/impl/CharStringStepper: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/convert/impl/ObjectArrayStepper: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/convert/impl/RangeStepper: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/generic/DefaultSerializationProxy: a supertype reference could not be resolved to a name" }=6
    Malformed { reason: "invalid .tasty file for scala/collection/immutable/BitSet: a supertype reference could not be resolved to a name" }=5
    Malformed { reason: "invalid .tasty file for scala/collection/immutable/Iterable: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/immutable/List: a supertype reference could not be resolved to a name" }=10
    Malformed { reason: "invalid .tasty file for scala/collection/immutable/WrappedString: a supertype reference could not be resolved to a name" }=11
    Malformed { reason: "invalid .tasty file for scala/collection/mutable/ArrayBuffer: a supertype reference could not be resolved to a name" }=4
    Malformed { reason: "invalid .tasty file for scala/collection/mutable/ListBuffer: a supertype reference could not be resolved to a name" }=15
    Malformed { reason: "invalid .tasty file for scala/collection/mutable/Queue: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/mutable/StringBuilder: a supertype reference could not be resolved to a name" }=7
    Malformed { reason: "invalid .tasty file for scala/concurrent/duration/Duration: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/math/Ordering: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/reflect/ClassTag: a supertype reference could not be resolved to a name" }=2
    Malformed { reason: "invalid .tasty file for scala/util/Try: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/util/matching/Regex: a supertype reference could not be resolved to a name" }=2
  most_requested_unresolved_member_names:
    tpd=731
    scala=640
    Contexts=519
    Int=399
    core=274
    CollectionConverters=270
    ast=225
    Array=212
    Type=151
    Boolean=108
    Symbol=105
    Trees=98
    Predef=91
    Tree=91
    String=88
    dotty=87
    Context=76
    reflect=74
    Types=70
    List=65
immutable_class_field_completion_outcomes:
  compiler/src/dotty/tools/dotc/core/TypeErrors.scala: tree=584 field=cycleSym baseline_occurrences=1 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 5 }
  compiler/src/dotty/tools/dotc/inlines/Inliner.scala: tree=966 field=thisProxy baseline_occurrences=1 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 5 }
  compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala: tree=74 field=debugPrint baseline_occurrences=1 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 20 }
  compiler/src/dotty/tools/dotc/reporting/Profile.scala: tree=231 field=pinfo baseline_occurrences=5 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 5 }
  compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala: tree=82 field=pbuf baseline_occurrences=2 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 10 }
  compiler/src/dotty/tools/dotc/transform/Bridges.scala: tree=152 field=bridgesScope baseline_occurrences=3 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 5 }
  compiler/src/dotty/tools/io/FileWriters.scala: tree=1129 field=isWindows baseline_occurrences=1 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 77 }
mutable_class_field_completion_outcomes:
  compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala: tree=895 field=curEnv baseline_occurrences=2 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 5 }
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala: tree=524 field=allowLeadingInfixOperators baseline_occurrences=1 outcome=completed
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala: tree=937 field=skipping baseline_occurrences=1 outcome=completed
  compiler/src/dotty/tools/dotc/reporting/Message.scala: tree=222 field=disambi baseline_occurrences=2 outcome=completed
  compiler/src/dotty/tools/dotc/typer/Applications.scala: tree=4151 field=typedArgBuf baseline_occurrences=1 outcome=blocked::ImportQualifierNotFound::ImportQualifierNotFound { source: SourceId(0), import_tree_index: 5 }
  compiler/src/dotty/tools/dotc/util/WeakHashSet.scala: tree=82 field=table baseline_occurrences=1 outcome=blocked::TypeNameNotFound::TypeNameNotFound { source: SourceId(0), tree_index: 71, name: Name { text: NameId(65), namespace: Type }, position: Some(SourceSpan { source: SourceId(0), span: Span { range: TextRange { start: 1815, end: 1820 }, point: None } }) }
  library/src/scala/collection/Iterator.scala: tree=3718 field=currentHasNextChecked baseline_occurrences=2 outcome=completed
immutable_class_field_baseline_method_outcomes:
  compiler/src/dotty/tools/dotc/core/TypeErrors.scala#tree=695 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/inlines/Inliner.scala#tree=1538 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala#tree=396 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=470 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=531 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=549 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=660 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Profile.scala#tree=811 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala#tree=235 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala#tree=292 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/Bridges.scala#tree=204 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/Bridges.scala#tree=212 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/Bridges.scala#tree=247 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/io/FileWriters.scala#tree=1156 outcome=ImportQualifierNotFound
mutable_class_field_baseline_method_outcomes:
  compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala#tree=5089 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala#tree=5879 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala#tree=1474 outcome=MemberLookup
  compiler/src/dotty/tools/dotc/parsing/Scanners.scala#tree=998 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Message.scala#tree=291 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/reporting/Message.scala#tree=349 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/typer/Applications.scala#tree=4248 outcome=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/util/WeakHashSet.scala#tree=708 outcome=TypeNameNotFound
  library/src/scala/collection/Iterator.scala#tree=3805 outcome=UnsupportedTypeTree::Annotated
  library/src/scala/collection/Iterator.scala#tree=3865 outcome=UnsupportedTypeTree::Annotated
local_method_signature_profile:
  total_occurrences=0
  distinct_files=0
  features:
  records:
inline_parameter_signature_outcomes_after_902:
  compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1125=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1440=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1224=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=1301=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=2407=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/transform/MegaPhase.scala#tree=2116=ImportQualifierNotFound
by_name_method_outcomes:
  compiler/src/dotty/tools/dotc/core/SymUtils.scala::instantiateCFT=ImportQualifierNotFound
  compiler/src/dotty/tools/dotc/typer/Typer.scala::cases=ImportQualifierNotFound
singleton_reference_profile:
  total_first_blockers=0
  profile_entries=0
  distinct_singleton_source_trees=0
  distinct_enclosing_declarations=0
  distinct_reference_shapes=0
  first_blockers:
  singleton_source_trees:
  enclosing_declarations:
  reference_shapes:
singleton_source_inventory:
  total_singleton_type_trees=2012
  literal_references=39 files=13 examples=[compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/core/Periods.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/dotc/transform/CheckUnused.scala]
  this_references=987 files=117 examples=[compiler/src/dotty/tools/backend/jvm/opt/FifoCache.scala, compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala]
  identifier_references=930 files=122 examples=[compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala, compiler/src/dotty/tools/dotc/core/MacroClassLoader.scala, compiler/src/dotty/tools/dotc/core/NamerOps.scala]
  selection_references=44 files=17 examples=[compiler/src/dotty/tools/dotc/Run.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Access.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Constant.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Signature.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Tree.scala]
  unsupported_references=12 files=1 examples=[library/src/scala/util/Try.scala]
  reference_shapes:
    Ident=930 files=122 examples=[compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/core/Contexts.scala, compiler/src/dotty/tools/dotc/core/MacroClassLoader.scala, compiler/src/dotty/tools/dotc/core/NamerOps.scala]
    Literal(Boolean)=33 files=13 examples=[compiler/src/dotty/tools/dotc/cc/CaptureSet.scala, compiler/src/dotty/tools/dotc/core/Periods.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/reporting/trace.scala, compiler/src/dotty/tools/dotc/transform/CheckUnused.scala]
    Literal(Int)=6 files=2 examples=[library/src/scala/NamedTuple.scala, library/src/scala/Tuple.scala]
    Other(Annotated)=12 files=1 examples=[library/src/scala/util/Try.scala]
    Select=44 files=17 examples=[compiler/src/dotty/tools/dotc/Run.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Access.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Constant.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Signature.scala, compiler/src/dotty/tools/dotc/semanticdb/generated/Tree.scala]
    This=987 files=117 examples=[compiler/src/dotty/tools/backend/jvm/opt/FifoCache.scala, compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala]
singleton_projection_baseline:
  baseline_observations=22
  no_longer_first_blocked_by_singleton_projection=22
  remaining_UnsupportedSingletonReference=0
  current_first_blockers:
    ImportQualifierNotFound=22
  methods:
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=2930 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=2955 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3022 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3107 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3155 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3313 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3331 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3396 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3468 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3577 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3619 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3799 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3817 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=3865 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4084 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4125 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4138 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4149 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4181 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4202 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4881 first_blocker=ImportQualifierNotFound
    compiler/src/dotty/tools/dotc/transform/CheckUnused.scala#tree=4981 first_blocker=ImportQualifierNotFound
```
