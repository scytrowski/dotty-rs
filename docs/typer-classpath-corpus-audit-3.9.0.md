# Typer classpath corpus audit: Scala 3.9.0

This normalized result was produced by two identical runs. It measures local-method typing reachability; `NoSuccessfulEnclosingMethodTyping` is a downstream count and is excluded from the ranked semantic gaps. Parser recovery, parser/namer failures, classpath misses, and unsupported Typer semantics remain separate buckets. The deltas are historical measurements, not a claim that moving a failure to a later bucket is a semantic improvement.

Run `tools/typer-classpath-corpus-audit/run` with the environment documented in `docs/typer-v0.1-compatibility.md` to regenerate this report.

Resolver request totals include every resolver call. External request counts exclude calls satisfied by source symbols; therefore each external request count reconciles with its success, unresolved, and error counts, while source reuse is reported separately.

```text
scala_revision=777528f19a58e794c9954a42f433373472ec57f8
files=1236
parser_failed_files=0
namer_failed_files=2
file_read_failed_files=0
file_read_failed_paths=
recovered_parser_files=72
audit_v1_comparison:
  files_attempted=1236 (delta=+0)
  local_declarations=23332 (delta=+114)
  local_methods=3813 (delta=+35)
  typed_local_methods=0 (delta=+0)
  ImportQualifierNotFound=2167 (delta=+121)
  UnsupportedExpression_total=583 (delta=+11)
  LocalBlockDeclarationDeferred=273 (delta=+1)
  NoSuccessfulEnclosingMethodTyping=515 (delta=+32)
local_definitions=23332
local_val_defs=19094
local_def_defs=3813
local_imports=227
local_classes=78
local_objects=60
local_type_defs=36
local_pattern_bindings=24
expression_forms:
  Ident=249876
  Select=117314
  Apply=87221
  Block=42605
  InfixOp=39431
  If=16249
  Parens=15380
  Match=7352
  Assign=5993
  Function=4913
  New=4825
  PrefixOp=4522
  TypeApply=4209
  InterpolatedString=3169
  Tuple=1341
  While=1303
  Throw=791
  Return=693
  ForDo=654
  ParsedTry=507
  Typed=372
  ForYield=160
  PolyFunction=0
  PostfixOp=0
  Try=0
parser_diagnostics:
  ParserDiagnostic::ExpectedExpression=308
  ParserDiagnostic::UnexpectedToken=162
  ParserDiagnostic::ExpectedToken=102
  ParserDiagnostic::UnsupportedSyntax=67
  ParserDiagnostic::UnboundPlaceholderParameter=2
  ParserDiagnostic::ExpectedType=1
local_defdefs=3813
typed_local_defdefs=0
unsupported_expression_total=583
local_block_declaration_deferred=273
no_successful_enclosing_method_typing=515
import_qualifier_not_found=2167
external_name_or_member_resolution_failures=97
failure_families:
  resolution/classpath environment=2264
  other=656
  unsupported expression syntax/semantics=583
  local declaration deferral=273
  parser/namer=26
  type relation/inference/completion=11
local_defdef_failures:
  ImportQualifierNotFound [resolution/classpath environment]: 2167 (235 files) [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypeLoader.scala]
  NoSuccessfulEnclosingMethodTyping [other]: 515 (33 files) [compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/jvm/GenericSignatureVisitor.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/TreeInfo.scala]
  UnsupportedExpression::Match [unsupported expression syntax/semantics]: 199 (71 files) [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/opt/BTypesFromClassfile.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala]
  UnsupportedExpression::InfixOp [unsupported expression syntax/semantics]: 146 (52 files) [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/GenBCode.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala]
  UnsupportedExpression::Parens [unsupported expression syntax/semantics]: 146 (59 files) [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/analysis/ProdConsAnalyzer.scala, compiler/src/dotty/tools/backend/jvm/opt/CallGraph.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala]
  LocalBlockDeclarationDeferred::import [local declaration deferral]: 139 (29 files) [compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/Run.scala]
  UnsupportedTypeTree [other]: 81 (25 files) [compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala]
  LocalBlockDeclarationDeferred::pattern definition [local declaration deferral]: 70 (21 files) [compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala]
  TypeNameNotFound [resolution/classpath environment]: 59 (24 files) [compiler/src/dotty/tools/MainGenericCompiler.scala, compiler/src/dotty/tools/dotc/core/tasty/CommentPickler.scala, compiler/src/dotty/tools/dotc/core/tasty/TreeBuffer.scala, compiler/src/dotty/tools/dotc/coverage/Serializer.scala, compiler/src/dotty/tools/dotc/util/Chars.scala]
  UnsupportedExpression::Function [unsupported expression syntax/semantics]: 49 (7 files) [compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala, library/src/scala/jdk/AnyAccumulator.scala, library/src/scala/jdk/DoubleAccumulator.scala, library/src/scala/jdk/IntAccumulator.scala, library/src/scala/jdk/LongAccumulator.scala]
  LocalBlockDeclarationDeferred::extension methods [local declaration deferral]: 26 (5 files) [compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/reporting/MessageRendering.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/typer/Applications.scala, compiler/src/dotty/tools/dotc/typer/Checking.scala]
  MissingDeclaredType [other]: 24 (11 files) [compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/core/TypeErrors.scala, compiler/src/dotty/tools/dotc/inlines/Inliner.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala, compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala]
  LocalBlockDeclarationDeferred::val/var definition [local declaration deferral]: 21 (8 files) [compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala]
  AnonymousClassInstantiationDeferred [other]: 18 (12 files) [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/cc/Capability.scala, compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/core/Definitions.scala, compiler/src/dotty/tools/dotc/core/Symbols.scala]
  MemberLookup [resolution/classpath environment]: 16 (9 files) [compiler/src/dotty/tools/dotc/core/Denotations.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/core/classfile/ClassfileParser.scala, compiler/src/dotty/tools/dotc/core/tasty/TastyPrinter.scala, compiler/src/dotty/tools/dotc/transform/Pickler.scala]
  NamerError::InvalidVisibilityQualifier::private [parser/namer]: 15 (1 files) [compiler/src/dotty/tools/dotc/typer/ProtoTypes.scala]
  UnsupportedExpression::ParsedTry [unsupported expression syntax/semantics]: 13 (7 files) [compiler/src/dotty/tools/dotc/ast/Positioned.scala, compiler/src/dotty/tools/dotc/core/TypeComparer.scala, compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala, compiler/src/dotty/tools/dotc/interactive/LogicalPackagesProvider.scala, compiler/src/dotty/tools/dotc/transform/Erasure.scala]
  NamerError::MalformedAstShape [parser/namer]: 11 (1 files) [compiler/src/scala/quoted/runtime/impl/QuoteMatcher.scala]
  SymbolResolution [resolution/classpath environment]: 11 (6 files) [compiler/src/dotty/tools/dotc/core/Decorators.scala, compiler/src/dotty/tools/dotc/core/SymbolLoaders.scala, compiler/src/dotty/tools/dotc/core/tasty/TastyPickler.scala, compiler/src/dotty/tools/io/FileWriters.scala, library/src/scala/collection/mutable/ListBuffer.scala]
  TermNameNotFound [resolution/classpath environment]: 11 (6 files) [compiler/src/dotty/tools/backend/jvm/BCodeIdiomatic.scala, compiler/src/dotty/tools/dotc/config/ScalaVersion.scala, compiler/src/dotty/tools/dotc/util/ClasspathFromClassloader.scala, compiler/src/dotty/tools/dotc/util/WeakHashSet.scala, library/src/scala/quoted/Quotes.scala]
  UnsupportedExpression::PrefixOp [unsupported expression syntax/semantics]: 11 (3 files) [compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/core/ConstraintHandling.scala]
  SymbolSourceKindMismatch [other]: 10 (2 files) [compiler/src/dotty/tools/dotc/parsing/Tokens.scala, compiler/src/dotty/tools/dotc/typer/Namer.scala]
  UnsupportedExpression::ForDo [unsupported expression syntax/semantics]: 10 (5 files) [compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/core/OrderingConstraint.scala, compiler/src/dotty/tools/dotc/core/tasty/TreePickler.scala, compiler/src/dotty/tools/dotc/transform/LambdaLift.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala]
  LocalBlockDeclarationDeferred::module definition [local declaration deferral]: 9 (2 files) [compiler/src/dotty/tools/dotc/ast/DesugarEnums.scala, compiler/src/dotty/tools/dotc/typer/Implicits.scala]
  LocalBlockDeclarationDeferred::type definition [local declaration deferral]: 8 (3 files) [compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/typer/Checking.scala]
  LocalMethodSignatureDeferred [type relation/inference/completion]: 8 (2 files) [compiler/src/dotty/tools/dotc/core/SymUtils.scala, compiler/src/dotty/tools/dotc/transform/MegaPhase.scala]
  StringLiteralTypingDeferred [other]: 8 (5 files) [compiler/src/dotty/tools/dotc/core/NameOps.scala, compiler/src/dotty/tools/dotc/core/TypeErrors.scala, compiler/src/dotty/tools/dotc/core/tasty/TastyPrinter.scala, compiler/src/dotty/tools/dotc/reporting/messages.scala, library/src/scala/util/Random.scala]
  UnsupportedExpression::InterpolatedString [unsupported expression syntax/semantics]: 5 (4 files) [compiler/src/dotty/tools/dotc/core/ConstraintHandling.scala, compiler/src/dotty/tools/dotc/core/Types.scala, compiler/src/dotty/tools/dotc/report.scala, compiler/src/dotty/tools/dotc/util/SimpleIdentityMap.scala]
  UnstableSelectionPrefix [type relation/inference/completion]: 2 (1 files) [compiler/src/dotty/tools/dotc/core/Contexts.scala]
  UnsupportedExpression::ForYield [unsupported expression syntax/semantics]: 2 (1 files) [compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala]
  UnsupportedExpression::Throw [unsupported expression syntax/semantics]: 2 (2 files) [compiler/src/dotty/tools/backend/jvm/BTypes.scala, library/src/scala/collection/immutable/Queue.scala]
  ApplicationCalleeNotMethod [type relation/inference/completion]: 1 (1 files) [compiler/src/dotty/tools/dotc/core/TypeErasure.scala]
top_semantic_gaps:
  1. UnsupportedExpression::Match: count=199, files=71, category=expression typing, examples=compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeSyncAndTry.scala, compiler/src/dotty/tools/backend/jvm/opt/BTypesFromClassfile.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala
  2. UnsupportedExpression::InfixOp: count=146, files=52, category=expression typing, examples=compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/jvm/GenBCode.scala, compiler/src/dotty/tools/backend/jvm/opt/BoxUnbox.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala
  3. UnsupportedExpression::Parens: count=146, files=59, category=expression typing, examples=compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/analysis/ProdConsAnalyzer.scala, compiler/src/dotty/tools/backend/jvm/opt/CallGraph.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/jvm/opt/CopyProp.scala
  4. LocalBlockDeclarationDeferred::import: count=139, files=29, category=local declaration support, examples=compiler/src/dotty/tools/backend/jvm/BCodeBodyBuilder.scala, compiler/src/dotty/tools/backend/jvm/BCodeHelpers.scala, compiler/src/dotty/tools/backend/jvm/BackendUtils.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/Run.scala
  5. UnsupportedTypeTree: count=81, files=25, category=simple source lowering, examples=compiler/src/dotty/tools/backend/jvm/BCodeUtils.scala, compiler/src/dotty/tools/backend/jvm/BTypes.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala
  6. LocalBlockDeclarationDeferred::pattern definition: count=70, files=21, category=local declaration support, examples=compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/dotc/ast/Desugar.scala, compiler/src/dotty/tools/dotc/ast/tpd.scala, compiler/src/dotty/tools/dotc/ast/untpd.scala, compiler/src/dotty/tools/dotc/cc/SepCheck.scala
  7. UnsupportedExpression::Function: count=49, files=7, category=expression typing, examples=compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala, library/src/scala/jdk/AnyAccumulator.scala, library/src/scala/jdk/DoubleAccumulator.scala, library/src/scala/jdk/IntAccumulator.scala, library/src/scala/jdk/LongAccumulator.scala
  8. LocalBlockDeclarationDeferred::extension methods: count=26, files=5, category=local declaration support, examples=compiler/src/dotty/tools/dotc/cc/SepCheck.scala, compiler/src/dotty/tools/dotc/reporting/MessageRendering.scala, compiler/src/dotty/tools/dotc/transform/PatternMatcher.scala, compiler/src/dotty/tools/dotc/typer/Applications.scala, compiler/src/dotty/tools/dotc/typer/Checking.scala
  9. MissingDeclaredType: count=24, files=11, category=other, examples=compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala, compiler/src/dotty/tools/dotc/core/TypeErrors.scala, compiler/src/dotty/tools/dotc/inlines/Inliner.scala, compiler/src/dotty/tools/dotc/parsing/Scanners.scala, compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala
  10. LocalBlockDeclarationDeferred::val/var definition: count=21, files=8, category=local declaration support, examples=compiler/src/dotty/tools/backend/ScalaPrimitives.scala, compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala, compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala, compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala, compiler/src/dotty/tools/dotc/ast/Trees.scala
top_gap_implementation_scope_notes:
  UnsupportedExpression::Match (199 occurrences, 71 files): first_slice=type the scrutinee, each case pattern, and each case body against one expected result type; owner=dotty-typer/src/typer/expression/mod.rs, with a focused pattern helper; prerequisites=the current Match/CaseDef AST and typed pattern representation; non_goals=exhaustivity checking, GADT refinement, and inferred match result unions
  UnsupportedExpression::InfixOp (146 occurrences, 52 files): first_slice=lower one infix node to the existing selected-member application path; owner=dotty-typer/src/typer/expression/mod.rs; prerequisites=operator name, receiver, and right operand already present in the AST; non_goals=precedence parsing or general extension-method search
  UnsupportedExpression::Parens (146 occurrences, 59 files): first_slice=type the enclosed expression and preserve the wrapper source span; owner=dotty-typer/src/typer/expression; prerequisites=the inner expression's ordinary typing context; non_goals=new syntax and semantic changes to the enclosed expression
  LocalBlockDeclarationDeferred::import (139 occurrences, 29 files): first_slice=resolve one local import qualifier and install its selector in the active block scope; owner=dotty-typer/src/typer/expression/blocks.rs; prerequisites=the classpath resolver and transactional scope updates; non_goals=local type/class/object declarations and wildcard semantics beyond existing imports
  UnsupportedTypeTree (81 occurrences, 25 files): first_slice=lower the encountered type-tree shape into the existing core type model; owner=dotty-typer/src/typer/type_projection.rs; prerequisites=the source type AST node and its named symbol/type metadata; non_goals=new parser syntax or broad type-model changes
resolver_metrics:
  resolver_package_requests=12806
  external_package_requests=7781
  external_package_successes=1965
  external_package_unresolved=5816
  external_package_errors=0
  source_package_reuse=5025
  resolver_member_requests=3975
  external_member_requests=3975
  external_member_successes=0
  source_member_reuse=0
  external_member_unresolved=3884
  external_member_errors=91
  distinct_packages=23
  distinct_classes=0
  distinct_members=0
  member_error_kinds:
    Malformed { reason: "class dotty/tools/dotc/CompilationUnit failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class dotty/tools/io/AbstractFile failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=5
    Malformed { reason: "class dotty/tools/io/ClassPath failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class dotty/tools/io/ClassRepresentation failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=3
    Malformed { reason: "class dotty/tools/io/ManifestResources failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class dotty/tools/tasty/TastyBuffer failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class dotty/tools/tasty/TastyReader failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class dotty/tools/tasty/TastyVersion failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/Function1 failed to load because scala/AnyRef failed: class not found: scala/AnyRef" }=1
    Malformed { reason: "class scala/collection/AbstractIterator failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/AnyStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/DoubleStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/collection/IntStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/collection/IterableOnce failed to load because scala/collection/Any failed: class not found: scala/collection/Any" }=2
    Malformed { reason: "class scala/collection/Iterator failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/collection/LinearSeq failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/LongStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/collection/Seq failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=5
    Malformed { reason: "class scala/collection/SortedMapOps failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/SpecificIterableFactory failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=3
    Malformed { reason: "class scala/collection/StepperShape failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/collection/convert/impl/BoxedBooleanArrayStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/convert/impl/CharStringStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/convert/impl/ObjectArrayStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/convert/impl/RangeStepper failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/immutable/List failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=3
    Malformed { reason: "class scala/collection/mutable/ListBuffer failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/collection/mutable/Queue failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=1
    Malformed { reason: "class scala/collection/mutable/ReusableBuilder failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=3
    Malformed { reason: "class scala/io/Codec failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=2
    Malformed { reason: "class scala/runtime/AbstractPartialFunction failed to load because java/lang/Object failed: class java/lang/Object failed to load because java/lang/Class failed: class java/lang/Class failed to load because java/lang/Object failed: circular inheritance involving java/lang/Object" }=3
    Malformed { reason: "invalid .tasty file for scala/collection/WithFilter: a supertype reference could not be resolved to a name" }=5
    Malformed { reason: "invalid .tasty file for scala/collection/convert/impl/CodePointStringStepper: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/collection/generic/DefaultSerializationProxy: a supertype reference could not be resolved to a name" }=3
    Malformed { reason: "invalid .tasty file for scala/collection/immutable/WrappedString: a supertype reference could not be resolved to a name" }=11
    Malformed { reason: "invalid .tasty file for scala/collection/mutable/StringBuilder: a supertype reference could not be resolved to a name" }=7
    Malformed { reason: "invalid .tasty file for scala/reflect/ClassTag: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/util/Try: a supertype reference could not be resolved to a name" }=1
    Malformed { reason: "invalid .tasty file for scala/util/matching/Regex: a supertype reference could not be resolved to a name" }=2
  most_requested_unresolved_member_names:
    scala=420
    tpd=336
    Contexts=278
    Int=259
    CollectionConverters=159
    Array=105
    ast=102
    Predef=69
    Symbol=61
    Type=60
    core=58
    dotty=58
    Arr1=56
    Tree=54
    String=53
    List=47
    Trees=45
    reflect=44
    Context=42
    tools=39
```
