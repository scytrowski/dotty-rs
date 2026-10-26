# Scala 3.9 cross-class TASTy and selected-call audit

Issue #1671 is a characterization of the current JVM/TASTy boundary. It does
not enable code generation through the paired JVM/TASTy planner and does not
change typing behavior.

## Pinned oracle

The fixture at `tests/fixtures/jvm_tasty_cross_class/Reader.scala` was compiled
with Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8` (compiler reports
`3.9.0-bin-SNAPSHOT-git-777528f`) and Temurin JDK 21.0.8+9. The fixture is
passed to `scalac` by its repository-relative path so the TASTy `SourceFile`
path records `tests/fixtures/jvm_tasty_cross_class/Reader.scala`.

The checked-in Scala oracle artifacts have these SHA-256 digests:

| Artifact | SHA-256 |
| --- | --- |
| `Counter.class` | `af6a9b0d4897714d8bdd61ae8e35e026c8cf9d90039b43e16029bc90432e5952` |
| `Counter.tasty` | `073c703b8eae3a677585ff458c02285b3dadbd748630d05fdb530b4bb079e287` |
| `Reader.class` | `1a5a3b5a2268dcab183b3984f50c6787385483b3ef81486b8bc55252d55fd9bd` |
| `Reader.tasty` | `be224c22feb92430c4fa8fe8df110f3bb6ead5f4b14ec8f92ea7d974aac271c6` |

The observed Scala class files use major version 61. `Reader.read` has JVM
descriptor `(Lexample/Counter;)I`; its constant-pool entry #21 is a
`CONSTANT_Methodref` to `example/Counter.get:()I`, and its bytecode invokes
that reference with `invokevirtual`. Those JVM references are distinct from
the source and TASTy names.

Each class has a separate TASTy 28.9 sidecar and class-level `TASTY` link:

| Class | TASTy UUID |
| --- | --- |
| `example/Counter` | `00d45667-bf83-69e8-0062-9f05d0f08f1b` |
| `example/Reader` | `0070a042-f9d1-e6ef-0095-57a1d02e6a1c` |

The Counter sidecar declares only `Counter`; Reader's sidecar declares only
`Reader`. In Reader's `read` definition, the formal type is represented by an
`IDENTtpt` containing ordinary `TYPEREF Counter` qualified by package
`example`. It is not a `TYPEREFdirect` link to Counter's declaration and does
not copy Counter's declaration into Reader's sidecar. The call body is
`APPLY(SELECTin(...))`; the selected name is the signed TASTy name
`get[Signed Signature(List(),scala.Int) @get]`, which identifies the zero-arg
method returning `scala.Int`. Its qualifier is `TERMREFdirect` to the `counter`
formal node. The trailing type in `SELECTin` is the method owner's type, not
the selection result type: it is a `SHAREDTYPE` reference to the formal's
`Counter` type node. Scala's unpickler uses this owner type together with the
signed method name to resolve the member. The test walks these nodes and
asserts that the shared reference points to that exact formal type node.

## One-session frontend and backend observations

The focused audit test constructs one Lexer → Parser → Namer → Typer session
for both declarations. In that session, the symbols are Counter `SymbolId(39)`,
Reader `SymbolId(40)`, Counter.get `SymbolId(45)`, Reader.read `SymbolId(49)`
and Reader.read's `counter` formal `SymbolId(50)`. These numeric IDs describe
this test session only; the test asserts semantic identity and ownership
relationships rather than treating allocation order as a contract.

The root API `validate_cross_class_method_ref` builds a borrowed, read-only
`ValidatedCrossClassMethodRef` from that same source index, typed bindings,
semantic store, typed arena, and complete JVM unit. It verifies the exact
`Reader.read` formal type target, both class declaration scopes, public
`Counter.get(): Int`, typed `Select` target, receiver `TermRef`,
source-to-typed roots, and matching JVM method plans. The proof exposes the
canonical class, method, formal, and typed-root IDs plus descriptor `()I`;
its borrows keep those identities tied to the supplied session. As with other
arena-backed IDs, callers must pass the source index, typed bindings, ASTs, and
JVM unit from one session; equal numeric IDs cannot authenticate a foreign
same-slot arena. It rejects a missing planned class, a substituted method
plan, or a JVM unit from another semantic store before any artifact operation.
This proof remains the JVM-side provenance audit. The paired TASTy batch also
uses the same canonical class/method identities before it lowers a cross-class
body reference.

The formal completes to the canonical package-qualified
`TypeRef(SymbolId(39))`; an unrelated type prefix is rejected even if the
target symbol is correct. Typing the body succeeds. Its typed root maps from
the exact source RHS through `SourceTypedIndex`; the selected function's
`TermRef` target is `SymbolId(45)` and its prefix matches the typed receiver
type. The receiver is an `Ident("counter")` whose unqualified `TermRef` target
is `SymbolId(50)`. Stale source/typed roots and missing Counter.get typed bodies
return typed errors. No spelling-based target reconstruction is involved.

The whole-source JVM backend path emits both classes from that frontend
pipeline. A Java harness constructs the generated Counter and calls the
generated Reader under `java -Xverify:all`; the test also reverses the two
source declarations and repeats compilation and execution successfully. This
establishes JVM-only cross-class execution for the characterized fixture.

The paired batch planner now validates each class against the complete JVM
unit, so a same-unit sibling can participate in a method signature. Issue
[#1673](https://github.com/scytrowski/dotty-rs/issues/1673) adds the
characterization source at
[`tests/fixtures/jvm_tasty_cross_class/ReaderLiteral.scala`](../tests/fixtures/jvm_tasty_cross_class/ReaderLiteral.scala),
where `Reader.read(counter: Counter): Int = 0`. Its real one-session TASTy plan
emits the Reader-owned `PARAM counter` with the pinned external-to-sidecar
`TYPEREF Counter` qualified by `TERMREFpkg example`. The Reader sidecar still
declares only Reader; it does not copy Counter declarations. The corresponding
JVM metadata is checked as `(Lexample/Counter;)I` against the full planned
class unit.

Issue [#1674](https://github.com/scytrowski/dotty-rs/issues/1674) adds the
narrow `Reader.read` body `counter.get()` to the same-source batch. The pickler
requires the exact planned public `Counter.get(): Int` symbol, one explicit
empty parameter clause, a `counter` receiver bound to Reader's canonical
formal, same-package planned class ownership, and exact source-to-typed tree
bindings. It emits the body as `APPLY(SELECTin(...))`, with the signed
zero-argument `get` name and a shared reference to the formal's `Counter` type.
It does not add `new`, fields, setters, overloads, nested calls, or arbitrary
external class references. A batch without the exact planned method identity
is rejected.

Issue [#1675](https://github.com/scytrowski/dotty-rs/issues/1675) integrates the
original `Counter + Reader` source through `emit_paired_jvm_tasty_batch` in one
frontend session. The whole-source contract checks `Reader.read` against its
canonical method and `Counter` parameter identities, including the
`(Lexample/Counter;)I` classfile descriptor. Before the four in-memory outputs
are returned, the artifact contract decodes both class files and sidecars,
checks each 65.0/28.9.0 pair and its own UUID link, and checks the Reader
`read` body contains exactly `aload_1; invokevirtual; ireturn`. That
`invokevirtual` must resolve to the planned source `Counter.get` method's JVM
owner, name, and descriptor. Reader's TASTy sidecar still owns only Reader;
Counter's constructor, field, `get`, and `set` ABI remains unchanged. A missing
Reader artifact, changed formal descriptor, mismatched Methodref, invalid
second class, or failed sidecar validation rejects the complete batch.
Independent source sessions produce byte-identical four-file manifests.
This is a bounded in-memory library/test path; file publication and executing
the generated Reader method are covered by separate contracts.

Issue [#1676](https://github.com/scytrowski/dotty-rs/issues/1676) executes the
actual four artifacts returned by `emit_paired_jvm_tasty_batch`. The JDK test
stages both Rust classfiles and their sidecars in a fresh directory, compiles
only a Java harness, then runs it with `java -Xverify:all`. `javap -p -s -c
-v` verifies Reader's major version 65, instance method descriptor
`(Lexample/Counter;)I`, and `invokevirtual example/Counter.get:()I`. Runtime
checks read values 7, 13, and -42 after mutations, then confirms that a second
Counter remains independent (values -5 and 99). The test compares the staged
classfile bytes with the original Rust artifacts after execution, so Java
compilation cannot replace either class. This remains a JDK-only runtime check
and does not claim Scala TASTy consumer compatibility; that is issue
[#1677](https://github.com/scytrowski/dotty-rs/issues/1677).

The #1673 signature test and #1674 full-body test have opt-in pinned Scala
3.9.0 `-from-tasty` checks controlled by
`DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1` and `SCALA39_ROOT`. The #1674 check
compiles the fixture with pinned Scala, stages only the matching Counter class
and sidecar beside the Rust-produced Reader sidecar, and asks Scala to rebuild
Reader. `javap` verifies the rebuilt method descriptor
`(Lexample/Counter;)I`.

## Negative-case evidence

The negative matrix below identifies existing rejecting layers and regression
coverage. Tests that validate backend plan invariants use their normal typed
fixture setup; they do not forge a source-to-typed mapping for this Reader
fixture.

| Case | Rejecting layer / established behavior | Existing focused coverage |
| --- | --- | --- |
| Same-spelled but unrelated class `get` | Reader still invokes Counter.get; an added `Decoy.get` does not change the selected owner | `cross_class_reader_call_compiles_and_runs_with_a_planned_counter` in `tests/jvm_source_compilation.rs`; canonical same-name backend identities are also covered by `common_planned_ancestor_rejects_object_only_and_foreign_same_name_inputs` in `crates/dotty-jvm/src/tests.rs` |
| Missing or nonplanned class reference | Paired source planner returns `ReferenceClassNotInJvmPlan` for Counter when validating Reader alone | The paired Reader test above; backend reference-plan cases in `reference_projection_rejects_unplanned_stale_and_noncanonical_targets` in `crates/dotty-jvm/src/tests.rs` |
| Wrong package / internal name and duplicate output locations | Oracle preserves `example/Counter` and `example/Reader`; artifact paths derive from package ownership and unsafe/conflicting paths are rejected | `same_simple_class_name_keeps_distinct_package_artifact_paths` in `tests/jvm_tasty_contract.rs`; `batch_publisher_rejects_unsafe_or_conflicting_paths_before_writing` in `tests/jvm_tasty_artifacts.rs` |
| Ambiguous method overload | Typer defers overloaded member selection instead of selecting by spelling or declaration order | `prefix_overloaded_member_is_reported_as_ambiguous` in `crates/dotty-typer/src/typer/mod.rs` |
| Primitive formal used as a receiver | Typer rejects `counter.get()` with an `Int` formal as `MemberNotFound` | `cross_class_call_reports_primitive_receiver_to_the_typer` in `tests/jvm_source_compilation.rs` |
| Private selected method | Typer resolves the source call, then JVM backend validation rejects the cross-class target with `UnsupportedJvmVisibility { visibility: Private }` | `cross_class_private_method_call_reports_its_first_backend_failure` in `tests/jvm_source_compilation.rs` |
| Stale selected target / wrong owner | JVM canonical target validation returns `UnknownMethod` for a stale method symbol and `WrongOwner` for another class's same-named method | `rejects_unknown_class_method_and_parameter_ids` and `same_named_method_from_a_different_owner_cannot_replace_the_planned_symbol` in `crates/dotty-jvm/src/tests.rs` |
| Missing planned TASTy target method | Pickler rejects the call when the same-source batch omits the exact `Counter.get` method identity | `cross_class_call_requires_the_exact_planned_target_method` in `tests/jvm_tasty_plan.rs` |
| Reversed source class order | Both orders compile and execute against the generated Counter and Reader | `cross_class_reader_call_compiles_and_runs_with_a_planned_counter` in `tests/jvm_source_compilation.rs` |
| Mismatched TASTy path / UUID | Artifact and Scala-consumer controls reject stale links and mismatched class/sidecar pairs | `stale_classfile_link_uuid_is_rejected_against_validated_tasty_header` in `tests/jvm_tasty_contract.rs`; `pinned_scala_consumes_both_rust_tasty_classes_and_executes_the_original_batch` controls in `tests/jvm_tasty_multiclass_scala_consumer.rs` |

The primitive and private cases above exercise the source pipeline and record
their observed first error. The other stale-symbol and wrong-owner cases are
internal-plan negatives, so testing them uses the existing backend fixture
APIs rather than modifying the canonical typed Reader tree.

## Reproduction and validation

Regenerate the oracle fixture from the repository root using the pinned
Scala/JDK checkout:

```sh
export JAVA_HOME=/home/vdrake6/.jdks/jdk-21
export PATH="$JAVA_HOME/bin:/path/to/scala39-source/dist/target/universal/stage/bin:$PATH"
mkdir -p /tmp/issue-1671-oracle
scalac -d /tmp/issue-1671-oracle tests/fixtures/jvm_tasty_cross_class/Reader.scala
```

The focused Rust coverage is:

```sh
cargo test --test jvm_tasty_cross_class_audit --locked
cargo test --test jvm_tasty_artifacts counter_reader_emits_four_complete_linked_cross_class_artifacts --locked
cargo test --test jvm_tasty_contract cross_class_call_artifact_rejects_a_different_methodref --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --test jvm_source_compilation \
  cross_class_reader_call_compiles_and_runs_with_a_planned_counter --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --test jvm_tasty_artifacts \
  linked_counter_reader_batch_executes_on_verified_jdk21 --locked
```

Both runtime tests require JDK 21 `java`, `javac`, and `javap` on `PATH`; the
artifact-batch test is included in the mandatory JDK CI job and fails rather
than skipping when `DOTTY_CLASSFILE_REQUIRE_JDK=1` and a tool is missing. The
full workspace quality checks remain the repository gates described in
`AGENTS.md`; no Scala corpus was rebuilt for this bounded audit.

## Pinned Scala consumer of the linked Counter/Reader batch (#1677)

The independent consumer gate compiles
[`tests/fixtures/jvm_tasty_cross_class/Consumer.scala`](../tests/fixtures/jvm_tasty_cross_class/Consumer.scala)
against only the four artifacts produced by `emit_paired_jvm_tasty_batch` from
the unchanged [`Reader.scala`](../tests/fixtures/jvm_tasty_cross_class/Reader.scala)
source and the pinned Scala runtime/compiler JARs. Each fresh source session is
staged under its own empty classpath root. The Scala compiler writes only
`consumer/Consumer$.class` and `consumer/Consumer.class`; neither Rust dependency
classfile is generated or copied into the client output. The runtime classpath
places the staged Rust artifacts before the client and Scala JARs, and
`java -Xverify:all` executes the original Rust `Reader.read` method.

The compiler classpath is the eight JARs verified by the existing oracle
provenance manifest: `scala3-compiler_3`, `scala3-library_3`, `scala-library`,
`tasty-core_3`, and `scala3-interfaces` version 3.9.0; `scala-asm`
9.9.0-scala-1; `compiler-interface` 1.12.0; and `util-interface` 1.11.5.
Compiler invocation uses that classpath for the compiler and uses the Rust
artifact root plus those JARs as the client's `-classpath`. Runtime uses the
Rust artifact root, the separate client output, and the same pinned JARs.
Archive inspection rejects any compiler/runtime JAR that itself contains
`example/Counter.class` or `example/Reader.class`.

The strict run was performed with Scala **3.9.0**, source revision
`777528f19a58e794c9954a42f433373472ec57f8`, and Temurin **21.0.8**. The compiler
JAR hashes are the pinned values in
[`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json). Both Rust
batches had byte-identical artifacts and distinct per-class TASTy links. The
independent [`Consumer.scala`](../tests/fixtures/jvm_tasty_cross_class/Consumer.scala)
input SHA-256 is
`71a5920f7ee003e7245c9b833c02d6796e976c6050e93f5ce2bfe6062f5ef558`:

| Artifact | SHA-256 | TASTy link UUID |
| --- | --- | --- |
| `example/Counter.class` | `db6dbba04450ccb864a759a36bdc29eacae989de1dd62ccfe491f599621019d4` | `00a25b5f2a9ec7ac0000000000008193` |
| `example/Counter.tasty` | `316afa2d30882e408341f64e7d093cff73c76b130f78318b2136b744c1170a69` | `00a25b5f2a9ec7ac0000000000008193` |
| `example/Reader.class` | `e4a8b90f30f433b260a19689c7d5380b913a44e55519d79db5fb226497e81913` | `000aa2942d1232c40000000000008194` |
| `example/Reader.tasty` | `15c02f4624ef2ef6048fc444ab876340865ad895f19f3166c5ee3cb6f75bb42a` | `000aa2942d1232c40000000000008194` |

The consumer classfile references the expected Rust ABI:
`Counter.<init>(I)V`, `Reader.<init>()V`,
`Reader.read(Lexample/Counter;)I`, and `Counter.set(I)V`. It reads 7 through
Reader, mutates the same Counter to 13 and reads again, then verifies a second
Counter remains at -5 while the first changes. The JVM prints `713` for each
fresh Rust batch. SHA-256 values for both classfiles and TASTy documents are
compared before and after Scala compilation and JVM execution; they remain
unchanged.

TASTy-based member lookup is proved independently for each class. A compile-only
copy of `Counter.class` with `get()I` removed still resolves `counter.get()` when
its matching Counter TASTy sidecar is present. A separate copy of
`Reader.class` with `read(Lexample/Counter;)I` removed still resolves
`reader.read(counter)` from Reader TASTy. Each matched classfile-only control
fails with the missing-member diagnostic, so the main consumer's Scala-shaped
success cannot alone stand in for sidecar consumption. Further per-class
controls reject a stale classfile TASTy UUID under `-Werror`, a missing sidecar,
a sidecar at the wrong package path, and a TASTy file with corrupt magic. Those
inconsistent files are used only for compilation controls and are never run.
The observed Scala 3.9.0 diagnostic text includes `is out of sync with its
TASTy file` for the changed UUID, `This .tasty file is missing` for absent or
misplaced sidecars, `value get is not a member of example.Counter` and `value
read is not a member of example.Reader` for the classfile-only controls, and
`could not be read` / `not a TASTy file` for corrupt magic. UUID mismatch,
missing-sidecar, path, and corrupt-file checks run with `-Werror` where Scala
reports a warning instead of a hard error.

Run the strict test with the pinned local inputs configured explicitly:

```sh
export JAVA_HOME=/path/to/temurin-21.0.8
export PATH="$JAVA_HOME/bin:$PATH"
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export COURSIER_MAVEN_ROOT=/path/to/cached/maven2
DOTTY_REQUIRE_SCALA_TASTY_CROSS_CLASS_CONSUMER=1 cargo test \
  --test jvm_tasty_cross_class_scala_consumer --locked -- --nocapture
```

The default workspace test skips this external Scala consumer unless the flag
is set. With the flag set, missing or mismatched Scala source, pinned JARs,
JDK tools, or a failed consumer/control is an error; it cannot be reported as a
successful structural-only run. This issue proves bounded interoperability for
this linked Counter/Reader shape. It does not claim general Scala dependency
compatibility or change source typing and production code generation.
