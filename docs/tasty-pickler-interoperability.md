# Source-to-TASTy independent interoperability gate

This gate checks actual Rust source-pipeline output with the pinned independent
Scala compiler and executes the class produced from that TASTy. It does not use
`tests/fixtures/tasty_pickler/tasty/example/Answer.tasty` as the candidate and
does not put an independently compiled `Answer.class` on the classpath.

The canonical, unmodified source is
[`Answer.scala`](../tests/fixtures/tasty_pickler/Answer.scala):

```scala
package example

class Answer:
  def value: Int = 42
```

The root integration test reads that checked-in source file, runs
Lexer → Parser → Namer → Typer, gets its exact `TreeId<Typed>` from the
`SourceTypedIndex`, builds the validated source plan and writes complete TASTy
bytes. It separately checks determinism in another source session, Rust 28.9.0
validation/round-trip, owner-qualified class/method identities, declared Int
syntax and the actual decoded Int RHS. The semantic unpickler confirms the
entered package/class/method identities and owner relations, and decodes both
the canonical Int result and literal constant type. A small test-only resolver
answers only the exact builtin Int symbol from `Definitions` when the
requested name/namespace match and the prefix is the Scala package reference;
it does not infer a classpath symbol from spelling. The unpickler does not
reconstruct a typed AST body.

## Pinned inputs

- Scala compiler **3.9.0**, source revision
  `777528f19a58e794c9954a42f433373472ec57f8`.
- TASTy **28.9.0**.
- Compiler artifact SHA-256 values from
  [`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json): Scala
  compiler/library/interfaces/tasty-core 3.9.0, scala-asm 9.9.0-scala-1,
  compiler-interface 1.12.0, util-interface 1.11.5.
- Runtime used for this verification: Temurin **21.0.8+9**, Linux x86_64. A JDK
  21 or newer with `java`, `javac`, and `javap` is required for `--runtime`.

The runner verifies the Scala source checkout revision, all pinned JAR hashes,
JDK 21+ availability and the exact compiler version before invoking it; it also
records the selected JDK release in fixture-generation provenance. It does not
download tooling. Configure local paths explicitly:

```sh
export SCALA39_ROOT=/tmp/scala3-3.9.0
export JAVA_HOME=/home/vdrake6/.jdks/jdk-21
export COURSIER_MAVEN_ROOT=/home/vdrake6/.cache/coursier/v1/https/repo1.maven.org/maven2
```

The dependencies are under
`$COURSIER_MAVEN_ROOT/{org/scala-lang,org/scala-lang/modules,org/scala-sbt}/...`
with versions/hashes listed in `provenance.json`.

## Exact invocation

From the repository root, this command runs the real source test and then invokes
the pinned oracle on the emitted file:

```sh
mkdir -p target/issue-1195/tmp
TMPDIR="$PWD/target/issue-1195/tmp" \
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
SCALA39_ROOT=/tmp/scala3-3.9.0 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
COURSIER_MAVEN_ROOT=/home/vdrake6/.cache/coursier/v1/https/repo1.maven.org/maven2 \
cargo test --test tasty_source_pickling --locked -- --nocapture
```

For an already produced candidate at `target/issue-1195/generated/example/Answer.tasty`,
run the independent gate directly:

```sh
python3 tools/tasty-pickler-oracle/run.py verify \
  --runtime target/issue-1195/generated/example/Answer.tasty
```

The runner first uses the bounded Rust inspector, then stages only the candidate
at `candidate/example/Answer.tasty` and compiles into a separate empty `compiled/`
directory. Its Scala steps are `dotty.tools.dotc.decompiler.Main` and
`dotty.tools.dotc.Main -from-tasty`; the classpath contains only the pinned Scala
libraries/compiler. It checks that no `.class` is adjacent to the candidate and
that the fresh `compiled/example/Answer.class` exists. `javap` must show
`public int value()`. A small Java harness calls
`new example.Answer().value()` and requires the value `42`. The runtime classpath
contains only the fresh compiler output, so the test cannot be masked by a source
or fixture class.

The `--runtime` path also corrupts the generated file's magic and requires both
the Rust inspector and the Scala `-from-tasty` compiler to reject it. The normal
oracle harness covers wrong package layout and Rust/Scala malformed-wire
rejection. Oracle configuration, incompatible revisions/versions, missing
artifacts, timeouts and nonzero subprocess exits are harness failures, never
successful negative controls. Setting `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE`
explicitly makes missing/incompatible tools fail the integration test; without
it the Rust integration remains environment-independent and makes no claim of
Scala acceptance.

## Result and scope

The 2026-10-16 run succeeded: Scala decompilation and `-from-tasty` compilation
passed; a fresh class was inspected and executed; `Answer.value() = 42`; both
corrupt-file controls were rejected. The candidate SHA-256 was
`2972c4e0205a0bbdfecd2a752f8614c9f29bb04eb66df615875204784bfba166`.

This proves independent Scala 3.9.0 acceptance and runtime behavior for the one
supported public class/default constructor/parameterless Int literal method
slice. It does not prove support for additional expressions, generic or inline
features, source positions, annotations or Scala TASTy embedded in JVM class
files. The Rust unpickler does not yet resolve this file's external `scala.Int`
return reference without a classpath resolver and does not reconstruct typed
method bodies. Classfile-side TASTy linkage and richer source forms remain
separate follow-up work.

## Sprint 2 multi-method gate (issue #1244)

The checked-in [`Operations.scala`](../tests/fixtures/tasty_pickler/Operations.scala)
is also compiled without edits through the real Lexer → Parser → Namer → Typer
pipeline. The root integration test hands the canonical class, constructor,
method and parameter symbols to an immutable `PicklePlan`, and supplies each
method's exact body root from the real `SourceTypedIndex`. It checks stable
bytes across two fresh source sessions, TASTy 28.9.0 validation, source-order
method declarations, the `identity` body reference to its exact formal, the
`use` call's signed `identity(Int): Int` selection and literal argument, the
forward `first` selection of `second`, and the explicit empty clause and zero
literal for `zero()`. The semantic unpickler confirms the owner-qualified
`identity` method and formal identities supported by its inverse.

Run the focused integration and independent Scala runtime gate with:

```sh
SCALA39_ROOT=/tmp/scala3-3.9.0 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
COURSIER_MAVEN_ROOT=/home/vdrake6/.cache/coursier/v1/https/repo1.maven.org/maven2 \
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
cargo test --test tasty_source_pickling operations_source_preserves_canonical_references_and_runs_from_fresh_scala_class --locked -- --nocapture
```

The test writes only the Rust-produced `Operations.tasty` into a fresh candidate
package tree. The pinned decompiler and `-from-tasty` compiler consume that file
and write the class into a separate empty directory. `javap` checks the class,
and a Java harness built against only that fresh output asserts `use() == 42`,
`identity(17) == 17`, `first() == 7`, and `zero() == 0`. The runtime path also
requires both the bounded Rust inspector and Scala compiler to reject corrupted
and truncated candidates. Missing tools, wrong revisions, unpinned JARs,
timeouts or compiler failures fail the explicitly enabled test; the default
test remains independent of local Scala tooling and makes no claim of external
acceptance.

The shared oracle also recognizes the expanded Operations fixture emitted by
the pickler crate's call-expression test. It runs the original `use`,
`propagate(17)`, `picks`, and `nested` assertions for that shape, keeping the
multi-argument and nested-call interoperability gate intact alongside this
Sprint 2 profile.

The existing focused pickler tests exercise malformed/foreign identities,
source-to-typed binding mismatches, unsupported call shapes, and validation
failures without publishing output. These remain complementary to the fresh
compiler/runtime gate: that gate demonstrates acceptance and execution for the
supported Operations subset, not broad Scala language compatibility.

## Sprint 3 combined control-flow and local-value gate (issue #1315)

The integration test reads the checked-in, pinned
[`Decisions.scala`](../tests/fixtures/tasty_pickler/Decisions.scala) literally
and runs the real Lexer → Parser → Namer → Typer → TASTy Pickler pipeline. The
fixture retains its existing `unit` and `disabled` methods as well as the
Sprint 3 methods; no methods are removed to make the pickler accept the class.
`SourceTyper::typed_local_binding_at` supplies the canonical `saved` local
symbol, typed `ValDef` and scope to the pickler plan.

The test constructs two independent compiler sessions and requires identical
complete bytes. It checks TASTy 28.9.0 validation, source-order method
declarations, canonical class/method/formal ownership and the exact
`TERMREFdirect` target at the emitted `saved` `VALDEF` address. The local is
also verified as the `stored` method's block statistic; the `IF` condition,
both branches, nested conditionals and literal tags remain in the emitted tree.
The semantic unpickler resolves only the canonical built-in Int, Boolean and
Unit identities and checks owner-qualified class/method identities. It does not
reconstruct the typed body or a source-level local symbol from TASTy.

Run the focused test and independent compiler/runtime gate with the pinned
Scala 3.9.0 checkout, verified artifacts and JDK 21 or newer:

```sh
SCALA39_ROOT=/tmp/scala3-3.9.0 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
COURSIER_MAVEN_ROOT=/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2 \
DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1 \
cargo test --test tasty_source_pickling decisions_source_serializes_control_flow_and_runs_from_fresh_scala_class --locked -- --nocapture
```

For the run on 2026-10-19, the Rust-produced candidate SHA-256 was
`0d477cc9bfb4db726e9961a929f062d5dc2132eeffa99941eee38582bf2125ab`. The
runner staged only that file as `candidate/example/Decisions.tasty`, decompiled
it with Scala 3.9.0, compiled it with `-from-tasty` into a separate empty
directory and inspected the newly emitted class with `javap`. A Java harness
loaded only that new output and passed `enabled`, both `choose` outcomes, all
three `nested` outcomes and both `stored` outcomes. The runner also confirmed
Rust and Scala rejection of corrupted and truncated candidate files. Its JSON
result reported `rust_wire`, `scala_decompile`, `scala_from_tasty`, `fresh_class`,
`javap`, all four corruption/truncation rejection flags, and runtime profile
`standard` as successful.

| Evidence | Result for `Decisions.tasty` |
| --- | --- |
| Rust TASTy 28.9.0 parser and validator | Passed; round-trip bytes unchanged |
| Source/typed identity and deterministic output | Passed in two separately built frontend sessions |
| Semantic unpickler | Passed for canonical builtins and owner-qualified class/method declarations |
| Pinned Scala 3.9.0 decompiler and `-from-tasty` | Passed on Rust-produced bytes only |
| Fresh JVM class and runtime assertions | Passed for Boolean, both `choose` branches, nested `if`, and local `saved` |
| Malformed candidate controls | Rust and Scala rejected corrupt-magic and truncated candidates |

This gate covers the pinned fixture and its tested Int/Boolean/Unit literal,
parameter, `If`, ordered `Block`, local `ValDef`, and direct local-reference
shapes. It does not establish general Scala 3 compatibility, assignment,
closures, generic/local-polymorphic definitions, classfile-embedded TASTy
linkage, or typed-body reconstruction by the semantic unpickler. Further work
should add new bounded source shapes only with corresponding canonical
identity checks and independent Scala/runtime evidence.
