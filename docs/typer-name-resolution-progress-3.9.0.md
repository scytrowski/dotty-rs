# Typer name resolution progress, Scala 3.9.0

Issue: [#1566](https://github.com/scytrowski/dotty-rs/issues/1566)

## Bounded completion check for #1620

I rechecked the five frozen #1620 roots from the pre-implementation baseline
`ad3268b6132ac04d80bf628ec4d9483f4f5ff1f6` against current `main`
`da8eb8ef4fd4f2d6793eb35e08286c8a5a7a85a2`, using the pinned Scala 3.9.0
checkout `777528f19a58e794c9954a42f433373472ec57f8`, Scala artifacts 3.9.0,
and JDK 21.0.8. The bounded manifest selects two source files, so the audit
attempts 13 methods; the five issue roots are listed separately in
[`typer-name-resolution-1620-final-roots-3.9.0.tsv`](typer-name-resolution-1620-final-roots-3.9.0.tsv).

All 13 methods stopped at `ImportQualifierNotFound` on the baseline. On current
`main`, all 13 advance to later first blockers: seven `TermNameNotFound`, two
`TypeNameNotFound`, and four `MemberLookup`. None is fully typed. The selected
`ProdConsAnalyzer.scala#tree=273` root contains the real
`sourceValue.insns.iterator.asScala` use and advances to
`TypeNameNotFound`. Its resolver trace now resolves the import qualifier to a
TASTy-backed `scala.jdk.CollectionConverters` Object. This is a cumulative
baseline-to-current-main comparison; it does not attribute the transition to
PR #1621 alone.

The focused import lookup regression separately resolves the inherited TASTy
declaration `IteratorHasAsScala`, preserving its canonical symbol and owner
`AsScalaExtensions`; the pinned Scala oracle accepts the iterator conversion
and rejects the unrelated String conversion. The corpus audit itself did not
materialize `CollectionConverters`' ModuleClass or record an
`IteratorHasAsScala` candidate for the selected method attempts. This evidence
therefore meets the bounded target of moving a real root past the qualifier
blocker while validating inherited symbol identity in the focused regression;
it does not show that the corpus methods apply implicit conversions or become
fully typed.

The final focused audit ran twice with identical normalized output (SHA-256
`3182eac569079de63aefefbef554357a92d44f61d69ab1b306679a50816b1b7b`). The
baseline focused output had SHA-256
`17f5c4fe71fe6189921b7568a45a85a5fc4dc2d2011fd5e865a81776b1cdd42c`.
Reproduce the current bounded run by creating a manifest containing the two
unique source paths in the TSV's `source_file` column and running:

```sh
mkdir -p target/issue-1620-final-runs target/tmp
cat > target/issue-1620-final-runs/five-roots.manifest <<'EOF'
compiler/src/dotty/tools/backend/jvm/BackendUtils.scala
compiler/src/dotty/tools/backend/jvm/analysis/ProdConsAnalyzer.scala
EOF
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1620-final-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-manifest target/issue-1620-final-runs/five-roots.manifest \
  --verify-determinism target/issue-1620-final-runs/five-roots-main.md
```


## Fresh `package-term` baseline after PR #1642

The audit was run serially from exact `main` SHA
`be9264c797555d99e93d5d6fbb82a2bed9f85efa` (merge commit #1642), against the
pinned Scala checkout `777528f19a58e794c9954a42f433373472ec57f8`, Scala
artifacts 3.9.0, and JDK 21.0.8:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-main-be9264-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-main-be9264-package-term.md
```

Both serial passes covered 60 files and 369 selected method attempts. Their
normalized method sidecars matched byte-for-byte (SHA-256
`402f273c9e33fdead90d3b83b51522a32c58379c639b27c66dba86ba4acfbd38`). Parsing
and naming succeeded for every file; zero methods were fully typed. Of the 369
frozen rows, 292 had target observations. Comparing the frozen first blocker
with this current baseline, 361 rows previously stopped at
`ImportQualifierNotFound`: 139 now reach `MemberLookup`, 69 reach
`TypeNameNotFound`, 152 reach other later blockers, and one row,
`library/src/scala/collection/convert/JavaCollectionWrappers.scala#tree=2047`,
still has `ImportQualifierNotFound`. The remaining 8 historical
`TermNameNotFound` rows now reach either `MemberLookup` (4) or
`UnsupportedExpression::InterpolatedString` (4). These are blocker transitions
across the frozen cohort, not a claim that PR #1642 alone caused them.

Current first blockers across the full selected cohort are:

| First blocker | Rows | Files |
| --- | ---: | ---: |
| `MemberLookup` | 143 | 35 |
| `TypeNameNotFound` | 69 | 18 |
| `UnstableSelectionPrefix` | 36 | 6 |
| `TermNameNotFound` | 20 | 6 |
| `WrongTypeNameKind` | 20 | 2 |
| `ObjectModuleClassUnavailable` | 18 | 1 |
| `SymbolResolution` | 15 | 6 |
| `LocalValueConformanceUnsupported` | 13 | 1 |
| Other blockers | 35 | 18 |

The compact row-level reconciliation, including exact selected `path#tree`
IDs, prior/current blockers, current attempt IDs, and observed qualifier stage,
is in [`typer-name-resolution-1566-main-be9264-package-term.tsv`](typer-name-resolution-1566-main-be9264-package-term.tsv).
The generated full focused report and raw logs remain under `target/`; they
are not checked into Git.

### Current increment: retain expression context when selecting a package member

The initial ASM-method candidate was too broad. Direct Typer probes using the
full pinned Scala classpath typed `AbstractInsnNode.getOpcode`,
`ClassReader.readByte`, and `ClassReader.readShort`. `readUTF8` also typed
without a `scala.*` wildcard; with that wildcard, `Array[Char]` instead failed
through `scala.Char` → `scala.AnyVal` → missing `scala.Matchable`. The Scala
3.9 oracle fixture validates the ASM calls, but they were not the first corpus
blockers.

The focused `LocalOpt` probe, using its nested package clauses and import,
reproduced the first failure: `collection.mutable` returned
`MemberNotFound(mutable)`, while `mutable.ArrayBuffer` and the fully qualified
`scala.collection.mutable.ArrayBuffer` both resolved. The package qualifier
had first been resolved in the expression's import context; package-member
selection then recomputed the full qualifier with only its lexical context and
lost that imported `collection` binding. Package selection now passes the
same `ExpressionContext` through the qualifier resolution path.

The regression lives in the Typer unit tests. Its positive case defines a
source-owned `scala.collection.mutable.ArrayBuffer`, imports
`scala.collection.mutable`, and types
`collection.mutable.ArrayBuffer.empty[Int]`. Its negative case confirms that
`collection.noSuchPackage` remains a `MemberNotFound`. The pinned Scala 3.9
compiler accepts the positive source and rejects the negative source with
E008.

On the real `LocalOpt.scala#tree=3134` method, this moves the first blocker
from `MemberNotFound(mutable)` at AST tree 2928 to
`InvalidInferredLocalValueType` at tree 2933, on line 939:
`val renumber = collection.mutable.ArrayBuffer.empty[Int]`. The inferred type
at that boundary is `MethodType([], ArrayBuffer[Int])`, not a value type. This
is progress for one real root; zero methods in `LocalOpt.scala` are fully
typed. The same deterministic 60-file / 369-method `package-term` cohort was
rerun twice after the change. Both runs again had 292 observed target rows and
zero fully typed methods; their normalized TSV hashes matched at
`bb8888c938165fd2b143b50875d9c93452f29e75d6dfaddff8b22ef64f943d61`. A
row-by-row comparison against the `be9264` baseline found exactly one first
blocker transition: `LocalOpt.scala#tree=2986` (selected method, attempt
`#tree=3134`) moved from `MemberNotFound` to
`InvalidInferredLocalValueType`. The other 368 rows retained their first
blocker and typed status. The compact per-method reconciliation is in
[`typer-name-resolution-1566-after-package-context-3.9.0.tsv`](typer-name-resolution-1566-after-package-context-3.9.0.tsv).

### Follow-up research: empty `MethodType` results

The same deterministic cohort has four rows in
`InvalidInferredLocalValueType` across three files. The two
`InlinerHeuristics.scala` rows are nested helpers in one `parse` attempt. A
focused three-file replay with temporary type tracing found the same immediate
shape at each failed local value: `MethodType([], result)` with `Plain` kind.
The corresponding source expressions are:

| Source location | Inferred expression | Origin |
| --- | --- | --- |
| `LocalOpt.scala:939` | `collection.mutable.ArrayBuffer.empty[Int]` | Scala 3.9 parameterless generic method |
| `InlinerHeuristics.scala:305` | `List.empty[Entry]` | Scala 3.9 parameterless generic method |
| `ClosureOptimizer.scala:498` | `before.getPrevious` | Java zero-argument method selected without `()` |

The pinned Scala 3.9 oracle accepts the first two forms without an argument
clause. A separate control accepts `def noClause[A]: List[A]` as
`F.noClause[Int]` and `def emptyClause[A](): List[A]` as
`F.emptyClause[Int]()`, while the opposite call shapes fail. So adapting every
empty `MethodType` would be incorrect. A trace inside `type_type_application`
shows that for both generic methods the `PolyType` already has
`MethodType([], result)` before type arguments are instantiated; substitution
does not introduce the empty method clause.

The owning classloader path is now identified. Its `DecodedTastyMethod`
flattens all term clauses into `parameters`, and `lower_tasty_method` always
wraps that list in `Type::Method`, including when the TASTy declaration had no
term clause. This erases the distinction needed by the Typer and is the
upstream cause for both Scala generic methods; it is not a Typer-only
adaptation bug. #1566 explicitly says to coordinate with the separate TASTy
work rather than rewrite classloader internals in this slice; the focused
classloader follow-up is [#1647](https://github.com/scytrowski/dotty-rs/issues/1647).

That classloader fix is necessary but will not by itself make these local
values infer. For a declaration with no clauses of either kind, the TASTy
unpickler represents `ExprType` as `Type::ByName`, while the source Typer's
completed signature for `def answer: Int` is the bare `Int` result. A
type-parameter-only method such as `def noClause[A]: A` instead keeps `A`
directly under `Type::Poly`, matching the semantic unpickler. Today
`widen_expression_type_journaled`
unwraps `Type::ByName` only for by-name *parameters*; it does not adapt a
selected method whose loaded signature carries the TASTy `ExprType` wrapper.
Also, `is_valid_inferred_value_type` explicitly rejects `Type::ByName`. So
after preserving the TASTy clause distinction, the next step must reconcile
that external method shape with the source Typer's result-type view, while
keeping a by-name parameter lazy and `def f(): T` inapplicable without `()`.
This identifies the immediate Typer-side blocker; it does not justify
unwrapping every `ByName` type globally.

The Java `getPrevious` case is also a zero-argument method, but its
no-parentheses adaptation remains a separate form until proven. No method in
the cohort is fully typed yet.

### Local implementation of classloader follow-up #1647

The classloader now records term-clause presence separately from its flattened
parameter list. The pinned `Methods.tasty` regression checks a no-clause
method, an explicit `()`, a one-parameter clause, and a polymorphic no-clause
method. A declaration with no clauses of either kind lowers as `Type::ByName`
(TASTy `ExprType`); a type-parameter-only method keeps its direct result under
`Type::Poly`, matching the semantic unpickler; explicit `()` stays
`Type::Method([])`. The existing `Both.tasty` loader assertion was updated to
expect its no-clause `make` as an `ExprType`.

The review correction also decodes the `IDENTtpt` wrapper around a method
type-parameter reference, preserving its address-based identity. The frozen
60-file / 369-method cohort still has 292 observed target rows and zero fully
typed methods, but the focused blockers moved. `LocalOpt.scala#tree=2986`
advanced from `InvalidInferredLocalValueType` to
`ApplicationArgumentConformanceUnsupported`; the two `InlinerHeuristics.scala`
roots moved to `MemberLookup`; `ClosureOptimizer.scala#tree=1707` remains at
`InvalidInferredLocalValueType`. Exact before/after rows are in
[`typer-no-clause-method-progress-3.9.0.tsv`](typer-no-clause-method-progress-3.9.0.tsv).
Both serial passes matched sidecar SHA-256
`dcec82c0cf9577201fc172fdb1ef2a6d6742ca4af9dc2d7c1cff55579aec0152`.

The next bounded slice implements Scala's Java nullary-method adaptation.
Classfile methods already carry `JAVA_DEFINED`, while TASTy and source methods
do not. In value position, a selected Java method with one plain empty clause
now produces a typed zero-argument `Apply`. Application and type-application
callee paths keep the selected method reference raw, so explicit `f()` syntax
is not applied twice. The focused classfile integration tests cover both
`op.instanceMethod` and `op.instanceMethod()` and assert the typed `Apply`
shape and stable repeated source mapping.

Pinned Scala 3.9 controls accept Java `f()` as `f` in value position, reject a
Scala `def f()` without `()`, accept a Scala parameterless `def f` without
`()`, and reject `def f` with `()`. This increment is limited to
`JAVA_DEFINED` methods with a single plain empty clause; it does not unwrap
TASTy `ExprType`, Scala explicit empty clauses, polymorphic methods, or
by-name parameters.

The same deterministic 60-file / 369-method cohort still types 0 methods, but
eight rows change first blocker. Of the four previously focused roots,
`LocalOpt.scala#tree=2986` advances from
`ApplicationArgumentConformanceUnsupported` to `IfConditionConformanceUnsupported`;
the two `InlinerHeuristics.scala` roots remain at `MemberLookup`; and
`ClosureOptimizer.scala#tree=1707` advances from
`InvalidInferredLocalValueType` to `UnstableSelectionPrefix`. Six more rows
move in `BCodeSkelBuilder.scala#tree=1968`,
`BCodeSkelBuilder.scala#tree=2025`, `BCodeUtils.scala#tree=157`, and
`LocalOpt.scala#tree=2834`, `#tree=2875`, and `#tree=3172`.
Exact transitions are recorded in
[`typer-java-nullary-adaptation-progress-3.9.0.tsv`](typer-java-nullary-adaptation-progress-3.9.0.tsv).
The two normalized cohort sidecars match byte-for-byte at SHA-256
`80816778c15ca48652087367eb1d9a3314440484cd645c55bcb692fe9d365b25`.

The focused `LocalOpt.addVar` path now gets past `varIns.getOpcode` and stops
at condition conformance; the method remains incomplete. The cohort result is
progress in the selected roots, not corpus completion: zero of 369 selected
methods are fully typed. A later blocker is credited only after the preceding
operation's signature shape and source-oracle controls are shown correct.

Validation for this Typer slice passed `cargo fmt --all -- --check`,
`python3 tools/check-frontend-boundaries.py`,
`cargo test -p dotty-typer --all-targets --locked`, and
`cargo clippy -p dotty-typer --all-targets --locked -- -D warnings`. The
focused corpus audit was run twice serially; its compact sidecars matched as
recorded above.

The independent review of #1650 found that this adaptation read a Java
declaration's canonical signature directly, bypassing receiver-view validation
for applied external generic classes. It now widens the selected reference
before inspecting the empty method clause. A JDK 21 classfile fixture for
`Opcodes<T>.get(): T` verifies that both `op.get` and `op.get()` preserve the
existing `ExternalGenericInstantiationDeferred` rejection instead of
producing a typed application with an uninstantiated `T`. This does not add
external generic Java support. After the fix, the 60-file / 369-method audit
again produced 292 observed rows, zero fully typed methods, and identical
sidecars at SHA-256
`80816778c15ca48652087367eb1d9a3314440484cd645c55bcb692fe9d365b25`.

The cohort result is progress in the focused roots, not corpus completion:
zero of 369 selected methods are fully typed. A later blocker is credited only
after the preceding operation's signature shape and source-oracle controls are
shown correct.

Validation for the local #1647 change passed `cargo fmt --all -- --check`,
`python3 tools/check-frontend-boundaries.py`,
`cargo test -p dotty-classloader --all-targets --locked`,
`cargo test -p dotty-tasty-unpickler --test methods --locked`,
`cargo test -p dotty --all-targets --locked`, classloader Clippy and rustdoc,
and `git diff --check`. The frozen cohort audit ran twice serially and matched
as above.

`InlineInfoAttribute.scala#tree=420` remains a separate case. Its per-file
audit cannot see the source declaration `InlineInfo` in sibling
`BTypes.scala`: each audited file gets a fresh `SemanticStore` and `Packages`
registry. The recorded `SourceClassCompletion`/classloader error is therefore
not evidence of a TASTy member-lookup defect. No classloader change is proposed
from this root.

## Bounded continuation: inherited members of external object wildcard imports

Issue [#1620](https://github.com/scytrowski/dotty-rs/issues/1620) starts from
the merged #1618 baseline `ad3268b6132ac04d80bf628ec4d9483f4f5ff1f6`.
When a wildcard import qualifier is an external TASTy-backed Object and its
local scope has no requested name, the Typer now asks its existing inherited
member lookup to inspect the canonical ModuleClass hierarchy. It returns the
canonical declaration symbol and owner. An absent name remains absent; a
successfully completed hierarchy stays cached on that miss, while completion
or lookup errors still roll back store, resolver and type-index state. This is
needed because repeatedly rolling back a successful completion caused the
same parent hierarchy to be loaded again for each imported name.

The focused regression uses the real Scala 3.9 library fixture and verifies
that `CollectionConverters.*` exposes the inherited TASTy Method
`IteratorHasAsScala`, owned by `AsScalaExtensions`. It also checks that
`asScala` is not invented as a direct member of `CollectionConverters`, that
an unrelated missing member stays unresolved, and that the first successful
hierarchy completion is reused. The pinned Scala positive and negative source
oracle is documented in
[`InheritedObjectWildcard.oracle.md`](../crates/dotty-typer/tests/fixtures/inherited-object-wildcard/InheritedObjectWildcard.oracle.md).
Implicit conversion application remains a separate Typer capability.

The deterministic pinned `package-term` audit rechecked the same 60 files and
369 methods twice. Its normalized cohort sidecars matched byte-for-byte
(SHA-256 `e812c812be1c29723935fd4deb9e6b40386233f9f4fed952c162174dfbc44985`).
Both passes had zero parser failures, zero namer failures, and zero fully typed
methods. This cohort is an earlier snapshot; the one-file audit below is a
separate measurement at a later code revision. The report observed 292 target
rows and 139 current
`ImportQualifierNotFound` blockers. Among the 161 frozen rows whose failed
qualifier is exactly `scala.jdk.CollectionConverters`, 120 still stop at
`ImportQualifierNotFound` and 41 now have a later first blocker. The five-row
reconciliation below samples the 120 unresolved rows; this increment did not
move those corpus roots.
The audit observed no materialized external ModuleClasses or module members,
so the focused inherited-member path was not exercised by those method roots.
Five exact affected roots and their current outcomes are recorded in
[`typer-name-resolution-1566-inherited-object-wildcard-3.9.0.tsv`](typer-name-resolution-1566-inherited-object-wildcard-3.9.0.tsv).
This is a bounded symbol-lookup result, not a claim that the #1566 method
cohort advanced or that `asScala` calls now type.

The CI focused smoke audit provides a separate signal: 17 of its 31 methods
move from `SymbolResolution` to the later `MemberLookup` blocker, and member
lookup errors fall from 15 to 0. No method becomes fully typed. The exact
deterministic output is recorded in
[`typer-focused-smoke-baseline-3.9.0.txt`](typer-focused-smoke-baseline-3.9.0.txt).
This shows that the new hierarchy lookup advances some real typer paths. At
that cohort revision, the broader #1566 target methods remained blocked at the
missing `CollectionConverters` qualifier.

### Follow-up research after PR #1621

In the earlier frozen `package-term` snapshot (code revision
`51660f527dbf587cf1c514732c599bf2b9b04910`), the 120 rows whose frozen failed
qualifier was `scala.jdk.CollectionConverters` still had category
`ImportQualifierNotFound`; the full 60-file / 369-method cohort typed zero
complete methods. That historical row-level TSV records the earlier
qualifiers; it does not give the current failed qualifier. The audit's
independent prefix observations do find
`scala.jdk.CollectionConverters` as a TASTy-backed Object, but the cohort does
not record a corresponding ModuleClass or module members. That observation
does not prove the source import's qualifier-typing path succeeded: it is a
separate classpath probe.

I replayed the focused import regression using the pinned Maven
`scala-library-3.9.0.jar`, the full JDK 21 and Scala 3.9 audit classpath, and
the nested `dotty.tools` / `backend.jvm` package shape used by compiler
sources. The direct qualifier lookup and inherited `IteratorHasAsScala`
member lookup both passed, as did typing a method body that references the
imported member. The checked-in TASTy fixture is not byte-identical
to the Maven artifact (6,293 vs. 6,272 bytes; SHA-256
`280f40f2282173f2e62b8eafcce04f809af524f6153071fe042fea2d6f742980` vs.
`32e89fbf8c615892631884b627ceba507e92a9e26fb866a05edb2692c84fdc11`), but
the exact-artifact replay shows that this difference alone does not prevent
the tested lookup.

I ran a one-file full-source audit at code revision
`51660f527dbf587cf1c514732c599bf2b9b04910` (pinned Scala checkout
`777528f19a58e794c9954a42f433373472ec57f8`) for the frozen
`BackendUtils.scala#tree=240` / `#tree=302` roots and temporarily instrumented
the audit harness to print the exact import tree on failure. Parsing and
naming passed. Both roots reach the local import at `BackendUtils.scala:84`,
`import asm.Opcodes.*` (`import_tree=201`, qualifier `asm.Opcodes`), then stop
at `ImportQualifierNotFound`; zero of the two methods are fully typed. The
resolver trace for the same method attempt resolves the term request
`scala.jdk.CollectionConverters` to a TASTy Object. So this full-source run
does reach that Object qualifier and moves these two roots past the frozen
`CollectionConverters` blocker, though it does not establish that any
inherited member from that wildcard import was selected.

The frozen-root to current-attempt reconciliation, including both revisions
and both qualifier outcomes, is recorded in
[`typer-name-resolution-1566-backendutils-opcodes-reconciliation-3.9.0.tsv`](typer-name-resolution-1566-backendutils-opcodes-reconciliation-3.9.0.tsv).
The linked five-row TSV above remains an immutable record of the earlier cohort
snapshot.

The current failing request is instead for `Opcodes` under the
`scala.tools.asm` package in the term namespace, which the resolver reports
as `package term member unsupported`. The classfile artifact contains
`scala/tools/asm/Opcodes.class`. The pinned Scala 3.9.0 compiler accepts the
same import shape and a use of one imported constant:

```scala
package dotty.tools
package backend.jvm
import scala.tools.asm
object OpcodesImport:
  import asm.Opcodes.*
  val firstOpcode: Int = ICONST_0
```

A negative control using `asm.NotAnOpcode.*` fails with E008, as expected. At
the audited code revision, the source import qualifier path asked for
`Opcodes` in the term namespace and did not retry in the type namespace. The
bounded follow-up below implements that fallback and records the new blocker
for these roots. The pre-change focused raw report and trace logs remain local
under `target/issue-1620-runs/`; they are not checked in. Temporary harness
instrumentation was removed after the run.

### Bounded follow-up: package-qualified classfile import qualifiers

Implementation commit `502c4657b21a51af1d375c2bacb6a1422f90a337` keeps the
existing term lookup first for import qualifiers, then retries a missing
selection in the type namespace. This retry is enabled only for source import
qualifier typing; other calls to the shared qualifier resolver retain their
previous behavior. The regression uses a classfile-backed Java class with a
`public static final int ICONST_0` field and types
`import scala.tools.asm.Opcodes.*` followed by a use of `ICONST_0`.

The one-file audit was rerun on that code revision with the same Scala
3.9.0 checkout and `BackendUtils.scala` manifest above. Parsing and naming
passed; the two frozen roots `#240` and `#302` still share attempt `#365`, and
both remain untyped. Their first blocker moved from
`ImportQualifierNotFound` to `StringLiteralTypingDeferred`. The resolver trace
records the expected term miss for `Opcodes`, then succeeds for the type
request with canonical classfile-backed `scala.tools.asm.Opcodes` of kind
`Trait`. Thus the import qualifier and its static wildcard members no longer
block those method attempts, although no method is fully typed yet. The raw
post-change audit is retained locally at
`target/issue-1620-runs/backendutils-after-opcodes.md`.

An independent review found that filtering only the final wildcard/selective
member still allowed an instance field to serve as an intermediate import
qualifier. The fix now applies the Java static-member rule at every selected
segment of an import qualifier, including locally scoped and resolver-provided
members. Regressions cover a non-static field and method in wildcard and
selective imports, plus a `public final` instance field used as a chained
qualifier; a static constant remains importable. The focused Scala audit was
rerun after this correction (`target/issue-1620-runs/backendutils-after-static-filter.md`):
the same two frozen roots still reach `StringLiteralTypingDeferred`, with zero
fully typed methods. Both roots include the string argument
`"lambdaDeserialize"` passed to `mv.visitInvokeDynamicInsn`; this points to
the next narrow research question, how the session should provide the
canonical `java.lang.String` type for ordinary string literals. It does not
show that the `Opcodes` import remains a blocker.

I reran the focused audit on the merged #1625 tree
(`c2a6fcf09d27ca1c36ccafb8ca28c303b8ad3e6b`); its two rows and first blockers
are unchanged. Inspection of the pinned Scala 3.9.0 source makes the remaining
semantic gap concrete: `Constants.Constant.tpe` maps `StringTag` to
`defn.StringType`, `Definitions.StringClass` is
`requiredClass("java.lang.String")`, and `TypeAssigner` assigns ordinary
literals a `ConstantType`. Here, `literal_type` still defers both UTF-8 and
UTF-16 string constants, `Definitions` has no canonical String identity, and
`constant_underlying_type` returns no widened type for them. The existing
String concatenation regression loads a classfile-backed `java.lang.String`
through its resolver but still expects a string literal to defer, so class
availability alone is not sufficient.

The classpath resolver does provide a safe route to that identity: resolve the
`java.lang` package, then request its `String` member in the type namespace.
The classloader enters loaded classes into their canonical package scope, and
the existing type-member lookup already checks that scope before asking the
resolver. The typer can validate the resulting class with the same structural
checks used by its String concatenation intrinsic: `Class` kind, classfile
origin, type namespace, and the exact `java.lang` → `java` → root package owner
chain. This avoids name-only symbol discovery and does not require inventing a
builtin String symbol.

The remaining design issue is where to perform and retain that lookup. The
current `literal_type` and `constant_underlying_type` helpers take `&self`,
while resolver materialization is mutable and participates in paired resolver
and semantic-store checkpoints. String literals are first represented as
`Type::Constant`, so a narrow implementation must make the canonical String
type available both for literal conformance and when that constant is widened
or compared later, without keeping a stale arena ID across rollback. The
focused audit fails on the `lambdaDeserialize` argument before it can say
whether the varargs application itself is supported.

The pinned `scala-asm` 9.9.0 classfile independently confirms `Opcodes` is a
public interface (`ACC_PUBLIC | ACC_INTERFACE | ACC_ABSTRACT`) and its opcode
fields are `ACC_PUBLIC | ACC_STATIC | ACC_FINAL`. This matches the import rule
exercised by #1625.

### Canonical String literals

The follow-up resolves `java.lang.String` through the session resolver and
accepts it only when it is a classfile-backed type symbol with the exact
`java.lang` → `java` → root package owner chain. It preserves ordinary and
UTF-16 string literals as exact `Type::Constant` values; constant conformance
and widening resolve the same canonical class on demand instead of retaining a
possibly stale arena ID. Missing or noncanonical String symbols remain
unsupported. The classpath-backed literal, widening, and conformance regression
passes, as does the no-resolver test that confirms the explicit deferral path.

The frozen `BackendUtils.scala#tree=240` and `#tree=302` roots both move from
`StringLiteralTypingDeferred` to `MissingDeclaredType`; neither becomes fully
typed. The resolver trace records canonical classfile-backed
`java.lang.String`. The first missing type is an inferred lazy field at
`BackendUtils.scala:122`, `serializedLamdaObjDesc`. This is earlier than a
determination of whether the `targetMethods*` varargs call itself types, so
no varargs support is claimed. The focused result is retained locally at
`target/issue-1620-runs/backendutils-string-after.md`.

### Inferred class `lazy val` follow-up

The current follow-up extends the existing transactional inferred-field path
to `lazy val` on ordinary source classes. The source `LAZY` flag stays on the
field; this adds type inference only and emits no lazy initialization or
runtime-lowering trees. Scala 3.9.0 accepts an acyclic forward reference to a
later lazy field, but reports E045 for inferred direct and mutual cycles. The
Typer's existing field recursion guard rejects those cycles with
`RecursiveInferredFieldType`; mutable lazy, module-class lazy, and
inline/given/implicit fields remain deferred. The pinned source fixture is
`crates/dotty-typer/tests/fixtures/class-lazy-fields/ClassLazyFields.scala`;
Scala 3.9.0 reports E045 on the two inferred cycle cases and accepts the
forward-reference and explicitly typed self-reference cases.

The focused `BackendUtils.scala` audit was rerun with this change. Both frozen
roots `#240` and `#302` move past the inferred lazy field's former
`MissingDeclaredType` blocker to `StringLiteralTypingDeferred`; zero of the two
methods are fully typed. This run measures only the lazy-field movement; without
canonical String literal handling, the next blocker remains
`StringLiteralTypingDeferred`. The focused report is retained locally at
`target/issue-lazy-research/backendutils-after-lazy.md`; this does not establish
a full-corpus count change.

Reproduce the one-file audit (the manifest selects the file, then the audit
intersects it with the frozen method roots):

```sh
mkdir -p target/issue-1620-runs target/tmp
printf '%s\n' compiler/src/dotty/tools/backend/jvm/BackendUtils.scala \
  > target/issue-1620-runs/backendutils.manifest
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1620-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-manifest target/issue-1620-runs/backendutils.manifest \
  target/issue-1620-runs/backendutils-focused.md
```

Reproduce the Scala positive oracle and negative control. The Scala checkout
is pinned to the revision above; cached Maven artifacts include compiler and
library 3.9.0 plus `scala-asm` 9.9.0-scala-1. Both compilations use pinned JDK
21.0.8:

```sh
mkdir -p target/tmp target/issue-1620-runs/oracle-out
SCALA39_CP="$(find target/scala39-maven -type f -name '*.jar' -print | sort | paste -sd: -)"
cat > target/issue-1620-runs/OpcodesImport.scala <<'EOF'
package dotty.tools
package backend.jvm
import scala.tools.asm
object OpcodesImport:
  import asm.Opcodes.*
  val firstOpcode: Int = ICONST_0
EOF
JAVA_HOME="$PWD/target/temurin21-1525" "$PWD/target/temurin21-1525/bin/java" \
  -Djava.io.tmpdir="$PWD/target/tmp" -cp "$SCALA39_CP" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CP" -d target/issue-1620-runs/oracle-out \
  target/issue-1620-runs/OpcodesImport.scala
cat > target/issue-1620-runs/OpcodesImportNegative.scala <<'EOF'
package dotty.tools
package backend.jvm
import scala.tools.asm
object OpcodesImportNegative:
  import asm.NotAnOpcode.*
EOF
JAVA_HOME="$PWD/target/temurin21-1525" "$PWD/target/temurin21-1525/bin/java" \
  -Djava.io.tmpdir="$PWD/target/tmp" -cp "$SCALA39_CP" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CP" -d target/issue-1620-runs/oracle-out \
  target/issue-1620-runs/OpcodesImportNegative.scala
```

The positive compilation exits successfully. The negative control exits
nonzero with E008: `value NotAnOpcode is not a member of scala.tools.asm`.

Reproduce the cohort audit with the pinned Scala checkout and cached artifacts:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1620-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1620-package-term.md
```

## Latest bounded continuation: tuple type projection after Scala aliases

This continuation starts from merged `main` at
`826219e912f96249bc7655816ba6ee561ad974a4` (PR #1606). The classloader now
resolves the explicit `scala.Cloneable` parent through the exact
`scala/package.tasty` alias to `java.lang.Cloneable`. The implementation code
for this measured candidate is commit
`8940022a09c88eee422bcb27ccd769d0fc748d41`.
Source tuple types such
as `(String, Int)` now project to an applied canonical `scala.Tuple2` type;
the type lookup is package-scoped, so a same-named local `Tuple2` does not
shadow the language tuple class. The fixture regression asserts canonical
symbol identity and both type arguments. Pinned Scala 3.9 positive and
negative compiler cases are recorded in
[`TupleTypes.oracle.md`](../crates/dotty-typer/tests/fixtures/tuple-types/TupleTypes.oracle.md).

The deterministic `package-term` audit used the same pinned 60 files and 369
methods in both runs. The normalized cohort sidecars matched byte-for-byte;
there were no parser or namer failures and zero fully typed methods. All three
frozen `InlineInfoAttribute.read` roots now load
`scala.collection.mutable.TreeMap` after the Cloneable alias is resolved, then
stop when loading canonical `scala.Tuple2`. The first-blocker category remains
`SymbolResolution`; the failing classloader target moved from `TreeMap` to
`Tuple2`. Pinned `Tuple2.tasty` reports an unresolved applied parent whose
selected type is `*:` with arguments `Long` and `scala.collection.immutable.Tuple`.
That parent projection is the next bounded classloader dependency. The
main-to-candidate first-blocker histogram is unchanged (`SymbolResolution`
97; all other frozen category counts unchanged), and the same 3 roots remain
untyped. Within the two-step implementation, the rows advanced from
`SymbolResolution` on `TreeMap`, through `UnsupportedTypeTree::Tuple`, to
`SymbolResolution` on `Tuple2`; this is a real dependency-chain advance, not a
fully typed result.

The deterministic 3-file / 31-method smoke audit and 77-file / 576-method
`object-module` cohort also matched their verification reruns. Neither had
parser or namer failures, and both typed zero complete methods.

The three-row exact reconciliation is in
[`typer-name-resolution-1566-cloneable-tuple-3.9.0.tsv`](typer-name-resolution-1566-cloneable-tuple-3.9.0.tsv).
The #1566 goal of fully typing a real method remains unmet.

The independent review of PR #1618 found that a selected parent owned by an
object could be mistaken for a package-qualified alias when the resolver
returned a matching `package$` candidate. The classloader now requires the
selected alias qualifier itself to carry a package-qualified TASTy reference
before trying that fallback. A callback-aware regression supplies a candidate
for `review.Owner.Base` and verifies that the package-alias resolver is never
called; the positive `scala.collection.mutable.Cloneable` alias regression
still passes. The `package-term` audit was rerun twice after this safeguard:
both normalized sidecars have SHA-256
`3539057b5a0394c545ece90444554492eeb3f17f66bf6562cf172b71bc5a3f27`, with
the same 60 files, 369 methods, no parser/namer failures, and zero fully typed
methods. Thus the review fix did not change the recorded #1566 cohort result.

Reproduce the focused audit with the pinned Scala checkout and cached artifacts:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-tuple-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-tuple.md
```

## Earlier increment: type Scala's synthetic `String.+`

This increment starts from merged `main` at
`1b71f494fb6db882a42f31378a24ae6551986be0` (the tree is the reviewed #1601
state, `78e77011f6cdbdc08dc024dcad27ab926fdea7e4`). Scala 3.9.0's pinned
compiler source declares `Definitions.String_+` as a final synthetic member of
`java.lang.String` with parameter `Any` and result `String`; it is not in the
JDK classfile member table. The typer now supplies that member only when
ordinary lookup finds no `+` and the receiver is the exact classfile-backed
`java.lang.String` package path. A source/TASTy class with the same short name
does not qualify, and an existing declaration is not shadowed. The selected
symbol is owned by that canonical String class and has the compiler-declared
`Any => String` method shape.

The focused regression checks symbol ownership/signature, an unrelated source
class negative control, explicit `name.+(desc)` before any infix use, and
rollback when typing the right operand fails before a successful retry. The
explicit selection check prevents the lookup from depending on whether a prior
infix expression installed the synthetic member. The pinned positive/negative Scala oracle and
reproduction command are in
[`StringPlus.oracle.md`](../crates/dotty-typer/tests/fixtures/string-plus/StringPlus.oracle.md).

The deterministic `package-term` audit compared the same 60 files and 369
methods at baseline `78e77011f6cdbdc08dc024dcad27ab926fdea7e4` and implementation
`41f55521412e8ea7250967e6a03ececc84146d33`. Both runs matched exactly, with no
parser or namer failures and zero fully typed methods. Exactly three frozen
rows advanced, from `MemberNotFound` to
`ApplicationArgumentConformanceUnsupported` while typing the enclosing
`InlineInfoAttribute.read#tree=420` attempt:

| Frozen row | New first blocker | Fully typed |
| --- | --- | --- |
| `InlineInfoAttribute.scala#tree=212` (`nextByte`) | `ApplicationArgumentConformanceUnsupported` | no |
| `InlineInfoAttribute.scala#tree=226` (`nextUTF8`) | `ApplicationArgumentConformanceUnsupported` | no |
| `InlineInfoAttribute.scala#tree=239` (`nextShort`) | `ApplicationArgumentConformanceUnsupported` | no |

The only aggregate movement was `MemberNotFound` 6 → 3 and
`ApplicationArgumentConformanceUnsupported` 1 → 4. This records blocker
progress only; the #1566 goal of fully typing a real method remains unmet. The
three row-level transitions are in
[`typer-name-resolution-1566-string-plus-3.9.0.tsv`](typer-name-resolution-1566-string-plus-3.9.0.tsv).
After the review fix for explicit selection, the deterministic `package-term`
sidecar remained byte-for-byte identical to both the prior implementation run
and its own verification rerun.

The deterministic 3-file / 31-method smoke audit matched its checked-in
baseline exactly, including every method row and blocker count. The separate
deterministic `object-module` cohort covered 77 files and 576 methods, with no
parser or namer failures; it typed zero complete methods. These focused results
do not claim full-corpus compatibility.

## Next blocker diagnosis after PR #1602

The post-merge deterministic `package-term` audit was run on code HEAD
`15912366d5491117277c8749d0a7ceb4c08e77fc` (merged as
`41c2c6803f7c9f31ac22672ae853fc719314d3aa`). Its two normalized cohort
sidecars matched each other and the preceding String-plus run byte-for-byte.
The three `InlineInfoAttribute.read` rows still stop at the `Some(name + desc)`
application, now with `ApplicationArgumentConformanceUnsupported`.

A temporary local trace of that application found an actual value type of the
canonical classfile-backed `java.lang.String`; the formal parameter type is
`Type::Error`, with message
`unresolved .tasty member type: tasty_symbol::decode could not reduce the declared type tree to a name`.
The relation error is `UnsupportedType` for that error type against String.
Pinned Scala 3.9 accepts `Some(name + desc)`, whose generated `Some.apply` has a
polymorphic value parameter `A` and result `Some[A]`. The classloader's
`tasty_symbol::decode_members` currently skips method `TYPEPARAM` entries and
reduces term parameter and result type trees to class names; it cannot retain
this binder reference. This is a TASTy member-signature projection gap, not a
reason to relax conformance or change String alias handling.

The alternate `CommandLineParser.scala#tree=397` candidate was also inspected:
its first local conformance failure is `ExternalGenericInstantiationDeferred`
when checking the generic `Nil` initializer against `List[String]`. It does not
provide a smaller typer-only route through the current generic classpath model.

No open issue covered semantic generic-method projection in this classloader
path; open #1569 profiles TASTy costs only. The coordination issue
[#1603](https://github.com/scytrowski/dotty-rs/issues/1603) now tracks the
bounded classloader/TASTy dependency. Keep #1566's fully-typed-real-method goal
unmet until one of its frozen roots actually completes.

## Generic TASTy method signatures

The follow-up investigation found that `scala.Some.apply` is generic in the
pinned Scala 3.9.0 TASTy, while the classloader discarded method type
parameters and reduced declared method types to names. The classloader now
preserves supported method type parameters and address-based parameter
references, including the applied result `Some[A]`; unsupported bounds remain
explicit. Decoder and loader regressions use the checked-in `scala/Some.tasty`
fixture and verify the same binder identity in the method argument and result.

The deterministic `package-term` audit was rerun twice over the same 60 files
and 369 methods. Both runs matched, with no parser or namer failures and zero
fully typed methods. The three `InlineInfoAttribute.read` cohort rows advanced
from `ApplicationArgumentConformanceUnsupported` to
`IfBranchJoinUnsupported` at `read#tree=420`; they remain untyped. The join
compares the `None` and `Some[String]` branches. Its relation error is
`ExternalGenericInstantiationDeferred` for `scala.Some`: TASTy class type
parameters and their parent substitutions are not modeled yet. The typer
cannot soundly decide this relation from the current external class metadata.
The updated row-level evidence is in
[`typer-name-resolution-1566-tasty-generic-method-3.9.0.tsv`](typer-name-resolution-1566-tasty-generic-method-3.9.0.tsv).

The deterministic 3-file / 31-method focused smoke cohort also changed after
the fix that preserves `scala.Any` as the universal top type in TASTy bounds.
`StringParsers.scala#tree=482` (`step`) advances from
`ApplicationArgumentConformanceUnsupported` to `TermNameNotFound`; its resolver
trace records an unresolved term lookup for `Int`. The two smoke runs matched
byte-for-byte. No focused method became fully typed. The checked-in smoke
baseline records this intentional first-blocker movement.

## Final-class branch comparison

`Some[String]` and `None` have distinct final class identities: `Some` is
declared final, and a Scala module class cannot be subclassed. The typer now
answers these cross-class subtype queries as false from that identity alone.
This keeps generic same-class comparisons deferred and does not approximate a
generic parent substitution. A focused regression covers both directions and
retains the deferred result for two different instantiations of the same
external generic class.

In the pinned `InlineInfoAttribute.read` attempt, the three frozen rows first
advance from `IfBranchJoinUnsupported` to `TypeNameNotFound`. The next
projection is `mutable.TreeMap[(String, String), MethodInlineInfo]`. See
[`typer-name-resolution-1566-final-class-join-3.9.0.tsv`](typer-name-resolution-1566-final-class-join-3.9.0.tsv).

## Explicit classpath subpackage imports

Scala parses `import scala.collection.mutable` as the qualifier
`scala.collection` plus an explicit `mutable` selector. When that selected name
is a classpath package rather than a member in an already populated scope, the
typer now resolves the child package path and checks package-object collisions
before accepting it as a qualifier. This is limited to explicit imports and
qualifier lookup; wildcard imports and ordinary term/type import semantics keep
their existing paths.

A regression uses the checked-in Scala library TASTy fixture and verifies that
the imported name resolves to the canonical package
`scala.collection.mutable`. The fixture does not include `scala.Cloneable`, so
loading `TreeMap` then fails explicitly with a classpath `SymbolResolution` for
that missing parent. In the pinned 60-file / 369-method `package-term` cohort,
two normalized runs matched byte-for-byte. Ten rows advanced: three
`InlineInfoAttribute.read` rows from `TypeNameNotFound` to `SymbolResolution`,
five other rows from `ImportQualifierNotFound` to `SymbolResolution`, and two
from `SymbolResolution` to `ApplicationArgumentConformanceUnsupported`. No
method became fully typed; the cohort still has 0 of 369 fully typed methods.
The row-level movements are in
[`typer-name-resolution-1566-imported-subpackage-3.9.0.tsv`](typer-name-resolution-1566-imported-subpackage-3.9.0.tsv).
The comparison starts from code commit
`caeb3413e652fb07a0d84a3f1499878a5fb22530` and measures the imported-package
implementation at `7218df7a35da4a095e46c58c1a4d8a3467092a4b`. Both descend
from #1605's head `499c42f90f`; that PR was merged to `main` as
`48d93bad25cd0d1895d9db08f68a4a9fedd62547`.

The deterministic 3-file / 31-method focused smoke gate now records the new
`scala.tools.asm` package and `Label` class observations and corresponding
resolver counts. Its two runs matched the updated checked-in baseline; it still
typed 0 of 31 methods.

The #1566 goal of fully typing a real method remains unmet. The next bounded
dependency is the Scala package alias `scala.Cloneable = java.lang.Cloneable`
when completing external TASTy parent types; this is a classloader alias
resolution blocker, not evidence that the package import remains unresolved.

Reproduce the deterministic audit with the pinned Scala 3.9.0 checkout and
cached dependencies:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-imported-subpackage-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-imported-subpackage.md
```

Reproduce the deterministic cohort comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-string-plus-final-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-string-plus-final.md
```

The smoke and object-module audits used the same environment:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-imported-smoke-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv \
  --verify-determinism target/issue-1566-imported-smoke.md

PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-string-plus-object-module-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort object-module \
  --verify-determinism target/issue-1566-string-plus-object-module.md
```

## Previous increment: select a module's unique `apply` method

This increment starts from merged `main` at
`7d6c3ed3502d80efb235f994e551792fd2b4bdbb`, after #1600. In the pinned Scala
3.9.0 source, `InlineInfoAttribute.scala:115` constructs `Some(name + desc)`.
`Some` resolves to the canonical `scala.Some` companion module, whose generated
polymorphic `apply[A](value: A): Some[A]` method makes the module callable.
The source typer now selects that method when a module-class callee has exactly
one callable `apply`, then uses the existing polymorphic application inference
and conformance path. It preserves the selected method's symbol identity. A
module without `apply` still reports `ApplicationCalleeNotMethod`; ambiguous
module `apply` candidates remain explicitly deferred.

The focused regression exercises generic `Factory.apply[A]`, checks the
selected source method identity and inferred `Int` result, accepts plain
`private apply` from its declaring module, rejects unrelated private/protected
apply methods, and confirms that a non-callable module is rejected. Other
non-public visibility forms remain unsupported by this implicit selection.
The deterministic `package-term` audit used
the same 60 files and 369 methods in both runs. The normalized reports and
369-row cohort TSVs matched exactly. Both runs had no parser or namer failures
and typed zero complete methods. Eight frozen cohort rows advanced, all from
`ApplicationCalleeNotMethod`:

| Frozen row(s) | New first blocker | Evidence reached |
| --- | --- | --- |
| `InlineInfoAttribute.scala#tree=212,226,239` (`nextByte`, `nextUTF8`, `nextShort`) | `MemberNotFound` | `Some(...)` selects its companion `apply`; the enclosing `read#tree=420` attempt next fails while typing `name + desc`. |
| `ScalaVersion.scala#tree=234,254,264,335` | `UnsupportedExpression::InterpolatedString` | `Failure(...)` in `parse#tree=394` proceeds past module `apply` and reaches the interpolated failure message. |
| `MacroClassLoader.scala#tree=115` | `OverloadedSelectionDeferred` | `Option(...)` in `makeMacroClassLoader#tree=165` proceeds past module `apply`; the enclosing attempt next defers an overloaded selection. |

Aggregate first-blocker counts changed from `ApplicationCalleeNotMethod` 8 to
0, `MemberNotFound` 3 to 6, `UnsupportedExpression::InterpolatedString` 0 to
4, and `OverloadedSelectionDeferred` 1 to 2. All other first blockers were
unchanged. These are measured blocker transitions, not fully typed real
methods. The eight changed rows are recorded in
[`typer-name-resolution-1566-module-apply-3.9.0.tsv`](typer-name-resolution-1566-module-apply-3.9.0.tsv).

The deterministic 3-file / 31-method smoke audit also remained byte-for-byte
stable across its two runs. Outside the frozen cohort,
`StringParsers.parseInt#tree=482` advances from `ApplicationCalleeNotMethod` to
`ApplicationArgumentConformanceUnsupported` after resolving module `apply`;
the pinned smoke baseline records this intentional later blocker.

Reproduce the deterministic cohort comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-some-apply-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-some-apply.md
```

The `MemberNotFound` for `name + desc` recorded by this increment was addressed
by the synthetic `java.lang.String.+` support above; the next investigation is
the resulting `ApplicationArgumentConformanceUnsupported` at the enclosing
`Some(name + desc)` application.

## Latest increment: adapt `if` expressions without `else` to `Unit`

This increment starts from merged `main` at
`91304853b7ffdc382107841414710de48e31e01f`, after #1598. The pinned source
`InlineInfoAttribute.scala:110` contains `if (hasSelf) nextUTF8()` without an
`else`. Before this change, the parser's synthetic zero-width `Unit` else was
joined with the `String` then branch. The typer tried to compare those nominal
types and stopped at `IfBranchJoinUnsupported` because `String` class info was
unavailable. Scala 3.9.0's typer oracle confirms that a no-else conditional
discards a non-`Unit` branch value, has type `Unit`, and warns about the
discarded value. An explicit `else ()` remains a two-branch join.

The source typer recognizes a parser-synthetic zero-width `Unit` else, including
when it is reached recursively through an `else if` chain. Each non-`Unit` then
value in that incomplete chain is wrapped as a typed block statement followed
by a canonical `Unit` literal, so the whole chain has type `Unit`. An explicit
`else ()` remains an ordinary two-branch join. The focused typer regression
checks direct and nested chains as well as the explicit-else type, and the
pinned oracle fixture is
[`IfWithoutElse.oracle.md`](../crates/dotty-typer/tests/fixtures/if-without-else/IfWithoutElse.oracle.md).
The initial implementation commit was
`f7f0ac67b9df1a272883c8648f84ca99bea460ba`; the review correction covering
incomplete `else if` chains is `aade2d4de2`.

The deterministic `package-term` comparison used the same 60 files and 369
methods before and after this increment and was repeated after the review
correction. Both final runs matched exactly, typed zero complete methods, and
had no parser or namer failures. Exactly the three `InlineInfoAttribute` rows
advance, with no other first-blocker changes:

| First blocker | Before (`913048`) | After (`f7f0ac`) |
| --- | ---: | ---: |
| `IfBranchJoinUnsupported` | 3 | 0 |
| `ApplicationCalleeNotMethod` | 5 | 8 |
| `MemberNotFound` | 3 | 3 |
| All other first blockers | unchanged | unchanged |
| Fully typed methods | 0 | 0 |

`InlineInfoAttribute.scala#tree=212` (`nextByte`), `#tree=226`
(`nextUTF8`), and `#tree=239` (`nextShort`) now pass the no-else branch join.
Their enclosing `read#tree=420` attempt next stops at
`ApplicationCalleeNotMethod`. This is measured blocker progress, not a fully
typed real method. The per-method rows and failing source branch are in
[`typer-name-resolution-1566-if-without-else-unit-3.9.0.tsv`](typer-name-resolution-1566-if-without-else-unit-3.9.0.tsv).
The deterministic focused 3-file / 31-method smoke audit also matches its
checked-in baseline exactly. The Scala 3.9.0 oracle fixture includes
`if (flag) "outer" else if (innerFlag) "inner"`; the typer output assigns the
whole chain `Unit` and warns for both discarded strings, while explicit
`else ()` remains `String | Unit`. After the review correction, all 884
`dotty-typer` unit tests and integration targets passed, as did Clippy,
rustdoc, formatting, and the frontend boundary guard.

Reproduce the cohort comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-if-unit-chain-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-if-unit-chain.md
```

The deterministic run above was executed after `aade2d4de2`; the two normalized
369-row cohort TSV outputs matched exactly and had no blocker-count changes
relative to the pre-correction result.

The next bounded investigation is `ApplicationCalleeNotMethod` in the
`InlineInfoAttribute.read#tree=420` attempt. Identify the exact application and
its canonical callee before choosing a fix; do not expand into general
application or overload support from this result alone.

## Latest increment: bounded `Int.&`

This increment starts from merged `main` at
`a92dec87aaee0b0aa24098e01c18d811dfd0cbb8`, after #1595. The pinned Scala
3.9.0 library declares `Int.&(x: Int): Int` in `library/src/scala/Int.scala`.
The source typer now registers that one exact canonical method identity beside
the existing bounded `Int` operators. It accepts `flags & 1` and `Int & Int`;
the real typer fixture asserts the selected `SymbolId`, result types, and
rejection of an unrelated source class. This increment does not add the
adjacent Byte, Short, Char, Long, OR, XOR, or shift overloads. The positive and
negative compiler oracle cases and reproduction command are recorded in
[`IntBitwiseAnd.oracle.md`](../crates/dotty-typer/tests/fixtures/int-bitwise-and/IntBitwiseAnd.oracle.md).
The implementation and focused regression fixtures were validated at
`2fe24ca588f3d9b4093082a35a6f22b95ccbe89e`.

The deterministic `package-term` comparison used the same 60 files and 369
methods before and after this change. Both runs typed zero complete methods and
had no parser or namer failures. Aggregate first-blocker counts changed only
for the three selected rows: `MemberNotFound` moved from 6 to 3 and
`IfBranchJoinUnsupported` moved from 0 to 3; all other counts stayed the same.
`InlineInfoAttribute.scala#tree=212` (`nextByte`), `#tree=226` (`nextUTF8`),
and `#tree=239` (`nextShort`) now type their `flags & 1` expressions and stop
later in the enclosing `read#tree=420` attempt at `IfBranchJoinUnsupported`.
This is verified first-blocker progress, not a fully typed real method. The
remaining three-row movement is recorded in
[`typer-name-resolution-1566-int-bitwise-and-3.9.0.tsv`](typer-name-resolution-1566-int-bitwise-and-3.9.0.tsv).

The focused 3-file / 31-method smoke audit remained deterministic and matched
its checked-in baseline exactly. Reproduce the `package-term` comparison with:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-bit-and-final-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" TMPDIR="$PWD/target/tmp" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-bit-and-final.md
```

The smoke comparison uses the same environment with
`--focused-manifest docs/typer-focused-smoke-manifest-3.9.0.tsv`, then runs
`tools/typer-classpath-corpus-audit/verify_focused_gate.py` against the pinned
baseline.

The `IfBranchJoinUnsupported` follow-up from this historical increment was
investigated and resolved by the newer no-else `Unit` adaptation section above.

## Latest increment: universal equality

This increment starts from `main` at
`ed3325e57395a315b5e8b78cd9a9c0ca0bbc379d`, after #1593. The pinned Scala
3.9.0 compiler synthesizes final `Any.==` and `Any.!=` methods. The source typer
now registers these canonical builtin symbols, resolves universal equality
alongside narrower source overloads, and rejects statically incompatible
primitive/reference operand pairs after bounded alias normalization. An
applicable source overload keeps its symbol identity; otherwise compatible
operands use the canonical universal method.

The Scala 3.9 oracle accepts equality on `Any`, Boolean, numerics within the
numeric group, and references, and rejects `Int == String` with “Values of
types Int and String cannot be compared with == or !=”. Focused tests cover
both operators, literal and widened numeric operands, `Any` with `Int`,
compatible aliases, inherited universal equality alongside narrower class
overloads, and incompatible primitive/reference pairs. Oracle fixture details
are recorded in
[`UniversalEquality.oracle.md`](../crates/dotty-typer/tests/fixtures/universal-equality/UniversalEquality.oracle.md).

The deterministic `package-term` comparison uses the same 60 files and 369
methods before and after the equality change. Both runs typed zero complete
methods and had no parser or namer failures. Aggregate first-blocker counts
were unchanged (`ApplicationArgumentConformanceUnsupported=1`,
`MemberNotFound=6`, `SymbolResolution=91`, all other blockers=271). However,
the three `InlineInfoAttribute.read` attempts progress past `version == 1` and
now stop at `flags & 1`; the blocker remains `MemberNotFound`. No full method
typing is claimed. Per-row movement is recorded in
[`typer-name-resolution-1566-universal-equality-3.9.0.tsv`](typer-name-resolution-1566-universal-equality-3.9.0.tsv).

## Previous increment: Scala and JVM array conformance

This increment starts from `main` at `844c8f4965992504cfa21598309d6650c90d2b9d`,
after #1591. It allows the canonical `scala.Array[T]` and `Type::JavaArray`
representations to conform when their element types are equivalent. Element
types remain invariant. The matcher verifies the canonical `scala.Array` class
by its type namespace and package owner chain, including classpath-loaded
symbols whose `scala` package is absent from the source package registry.
Unrelated classes named `Array` do not receive this relation.

Focused unit controls cover `Array[Char]` against `char[]`, reject mismatched
element types and `custom.Array[Char]`, preserve `Array[Char] <: Object`, and
reject a `scala.Array` symbol with a noncanonical package prefix. A Scala 3.9.0
compiler oracle accepts passing `Array[Char]` to a Java `char[]` parameter and
rejects `Array[Byte]` at that call site.

The deterministic `package-term` comparison uses the same 60 files and 369
methods before and after this change. Both runs typed zero complete methods and
had no parser or namer failures. Three rows move forward without regressions:

| First blocker | Before (`844c8f`) | After this increment |
| --- | ---: | ---: |
| `ApplicationArgumentConformanceUnsupported` | 4 | 1 |
| `MemberNotFound` | 3 | 6 |
| `SymbolResolution` | 91 | 91 |
| All other first blockers | 271 | 271 |
| Fully typed methods | 0 | 0 |

`InlineInfoAttribute.scala#tree=212` (`nextByte`), `#tree=226` (`nextUTF8`),
and `#tree=239` (`nextShort`) now pass the array argument check. Their enclosing
`read` attempt next stops at `MemberNotFound` for `==` in `version == 1`.
This increment does not claim a fully typed real method. Per-row results are in
[`typer-name-resolution-1566-array-conformance-3.9.0.tsv`](typer-name-resolution-1566-array-conformance-3.9.0.tsv).

The report below records earlier #1566 increments and their historical
baselines; its “current” labels refer to those recorded runs, not this latest
array-conformance result.

## Baselines and increments

The cumulative comparison starts from `main` at
`e06c4eeea02471f674c6b4c0263eaf69f3bc1f60`, before #1577 was merged. PR #1577
added the Java static-qualifier increment, handling an imported Java class
used as a qualifier for static members. Its historical Scala 3.9 oracle check
accepted `Paths.get("input.txt")` after importing `java.nio.file.Paths`, and rejected
`Paths` as a value and instance-method selection through the class. Those
results describe that earlier increment, not this PR.

The historical pre-increment baseline for the mutable-local `+=` change is
`main` at
`bfc4759ea7d4eca10767ef5021ffc8386178c4a5`; it includes #1586, #1577, #1571,
and #1574. That increment added a bounded fallback for `+=` on a mutable local `Int`:
if direct `+=` member lookup misses and no visible extension candidate takes
precedence, it types `value + rhs` and lowers that result to assignment while
preserving the local symbol identity. The pinned inputs are Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and JDK
21.0.8.

## Frozen-cohort result

The pinned `package-term` cohort covers 60 files and 369 real methods. The
fresh pre-increment baseline and latest run each attempted all 369 methods;
both typed zero methods. That earlier increment advanced three rows with no
first-blocker regressions. The normalized reports and resolver sidecars for that run
matched byte-for-byte across two runs; neither run had parser or namer
failures.

### Historical `+=` comparison: `bfc4759` to that increment

| First blocker | Pre-increment rows | Current rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 1 | 1 |
| `MemberNotFound` | 6 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

| Source method | Pre-increment blocker | Current blocker | Attempt |
| --- | --- | --- | --- |
| `InlineInfoAttribute.scala#tree=212` (`nextByte`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |
| `InlineInfoAttribute.scala#tree=226` (`nextUTF8`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |
| `InlineInfoAttribute.scala#tree=239` (`nextShort`) | `MemberNotFound(+=)` | `SymbolResolution` (`scala.Array` parent) | `#tree=420` |

The rewrite types the mutable local integer update in each method. At that
point, their next failure was loading `scala.Array`, whose TASTy parent
`java.io.Serializable` remained unresolved. The latest increment above moves
past that boundary but still does not fully type these methods.

### Historical cumulative snapshot: `e06c4ee` to the `bfc4759` increment

Four rows advanced across the earlier increments captured in the compact TSV:
one in merged #1577 and three in the mutable-local `+=` change. This table
compares the historical baseline with the result at that earlier increment;
it is not the `bfc4759` pre-increment comparison above.

| First blocker | Historical baseline rows | Current rows |
| --- | ---: | ---: |
| `TermNameNotFound` | 2 | 1 |
| `MemberNotFound` | 5 | 3 |
| `SymbolResolution` | 97 | 100 |
| All other first blockers | 265 | 265 |
| Fully typed methods | 0 | 0 |

The complete per-method before/after reconciliation for that historical
snapshot is in
[`typer-name-resolution-1566-cohort-3.9.0.tsv`](typer-name-resolution-1566-cohort-3.9.0.tsv).
Its `before_*` columns refer to the historical `e06c4ee` baseline; its
`after_*` columns refer to the current implementation.
The earlier default Scala package-term increment remains recorded in
[`typer-name-resolution-progress-3.9.0.tsv`](typer-name-resolution-progress-3.9.0.tsv).

## Previously recorded bounded blockers

The next failure in `expandArg` is Scala's `String.stripPrefix` extension
method. The pinned Scala source defines `Predef.augmentString` with the alias
`Predef.String` as its parameter (`library/src/scala/Predef.scala:171`). The
classpath resolver finds the canonical `scala.Predef$.augmentString` method,
but the current TASTy member-type lowering treats its parameter name
`scala/Predef$String` as a JVM class. No such class exists, so the parameter is
represented as `Type::Error` with the loader diagnostic `class not found:
scala/Predef$String`. Replacing that error with `java.lang.String` by spelling
would manufacture a type instead of resolving the alias, so no implicit-view
call is synthesized. Correct support needs alias-aware TASTy member-type
resolution; general implicit search remains outside this bounded increment.
At that time, the three advanced methods encountered a separate classpath boundary:
decoding `scala.Array` fails on its selected-qualified `java.io.Serializable`
supertype reference. The loader leaves semantic references unresolved when it
cannot prove their owner path; this increment does not add a spelling-based
package guess.

The fresh pre-increment baseline at `bfc4759ea7d4eca10767ef5021ffc8386178c4a5`
and the latest implementation each audited all 60 files and 369 methods twice
with deterministic normalized reports and sidecars. In the latest report,
`CommandLineParser.scala#tree=467` still stops at `MemberNotFound` for
`String.stripPrefix`; the three `InlineInfoAttribute` rows stop at
`SymbolResolution`. The compact TSV records the cumulative before/after rows.
No full-corpus result is claimed.

## Reproduction

The Scala oracle command below validates the earlier Java static-qualifier
increment, not this PR. Run it with the pinned artifacts by setting `JAVA_HOME`
and `SCALA39_CLASSPATH`, then:

```sh
python3 tools/typer-classpath-corpus-audit/scala39_import_oracle.py
cargo test -p dotty-typer --test refactor_contract java_class_import_qualifier --locked
```

The deterministic pre-increment baseline audit used:

```sh
PYTHONUNBUFFERED=1 DOTTY_AUDIT_TMP_ROOT="$PWD/target/issue-1566-latest-baseline-runs" \
SCALA39_ROOT="$PWD/target/dotty-scala39-1525" JAVA_HOME="$PWD/target/temurin21-1525" \
SCALA39_JDK_RELEASE=21 COURSIER_MAVEN_ROOT="$PWD/target/scala39-maven" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
tools/typer-classpath-corpus-audit/run --focused-cohort package-term \
  --verify-determinism target/issue-1566-latest-baseline.md
```

The deterministic audit for the array-conformance increment used the same
command with `target/issue-1566-array-audit-final.md` as the output and
`target/issue-1566-array-audit-final-runs` as `DOTTY_AUDIT_TMP_ROOT`. The
universal-equality increment used `target/issue-1566-equality-final.md` and
`target/issue-1566-equality-final-runs`; both normalized reports and resolver
sidecars matched across runs. The equality audit was run after removing
temporary diagnostics. Its result shows the same first-blocker counts while
the three noted expressions advance from `version == 1` to `flags & 1`.

The CI focused smoke manifest was also rerun twice after the equality
increment. Its normalized output is deterministic and is recorded in
[`typer-focused-smoke-baseline-3.9.0.txt`](typer-focused-smoke-baseline-3.9.0.txt).
In the selected 31 methods, no method becomes fully typed. Three
`StringParsers.scala` rows move from `MemberNotFound` to `MemberLookup`,
`ApplicationCalleeNotMethod`, and `MemberLookup`; `scala.Some` is now
materialized while resolving the affected branch. The aggregate selected
blockers move from 11 to 12 `ApplicationCalleeNotMethod`, from 3 to 0
`MemberNotFound`, and add 2 `MemberLookup` rows. This is audit progress, not a
claim that the surrounding methods now type completely.
