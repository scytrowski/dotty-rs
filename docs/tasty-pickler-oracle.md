# Scala 3.9 semantic pickler oracle

This is the independent fixture and compiler harness for
[#1190](https://github.com/scytrowski/dotty-rs/issues/1190). It fixes the observed
wire structure and the remaining metadata decision in the
[pickler design](tasty-pickler-design.md). It does not implement a semantic pickler
or change the source frontend, semantic store, JVM backend, or TASTy codec.

## Pinned inputs and fixture inventory

The compiler is Scala **3.9.0**, source revision
`777528f19a58e794c9954a42f433373472ec57f8`, emitting TASTy **28.9.0**. The recorded
generation/verification used Temurin **21.0.8+9** on Linux x86_64. Normal Rust CI
reads checked-in files and needs no Scala installation or network access.

[`provenance.json`](../tests/fixtures/tasty_pickler/provenance.json) records source
and TASTy SHA-256 values, compiler dependency hashes, JDK identity and the exact
generation command. The harness verifies the source checkout revision, compiler
version and all eight cached JAR hashes. It does not download dependencies.
JAR hashes are checked before invoking the compiler, including its version probe.
Scala's source checkout and release artifacts are separate inputs: both are
pinned, rather than assuming a version string alone authenticates the compiler.

Fixtures are under [`tests/fixtures/tasty_pickler/`](../tests/fixtures/tasty_pickler):

| Source / output relative to `tasty/` | Observed facts pinned by snapshots |
| --- | --- |
| `Answer.scala` / `example/Answer.tasty` | `PACKAGE example`; class `TYPEDEF`/`TEMPLATE`; implicit Object parent construction; default constructor; parameterless `value`, explicit Int result, actual `INTconst 42` RHS |
| `Empty.scala` / `Empty.tasty` | Default package (`TERMREFpkg <empty>`), no source methods, compiler-generated constructor |
| `Parameters.scala` / `example/Parameters.tasty` | `pick(first: Int, second: Int): Int = second`: ordered PARAM definitions owned by `pick`; RHS `TERMREFdirect` targets **second**; `zero()` retains EMPTYCLAUSE |
| `Declarations.scala` / `example/Declarations.tasty` | ABSTRACT class; `missing: Int` and `explicit(): Int` have **no RHS**; absent clause versus empty clause remains observable |
| `References.scala` / `example/References.tasty` | A source reference to a later method is a **named SELECT**, not an AST-address forward reference; its qualified `this` has a `TYPEREFsymbol` targeting this exact class definition |

Each source produces exactly one top-level `.tasty`. Adjacent `.class` files are
generated in temporary compiler output but are not checked in or supplied to the
standalone reader. The additional fixtures characterize grammar/ref distinctions;
they do not broaden the minimal producer's supported language subset.

The underlying rules were checked against the pinned upstream
[`Pickler.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/transform/Pickler.scala),
[`TreePickler.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/tasty/TreePickler.scala),
[`TastyPickler.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/tasty/TastyPickler.scala)
and [`NameBuffer.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/tasty/NameBuffer.scala).
In particular, DEFDEF writes parameters, return tree, optional RHS and modifiers;
it does not attach a second METHODtype signature to every declaration.

## Frozen minimal Answer shape

The machine-readable [Answer snapshot](../tests/fixtures/tasty_pickler/snapshots/example/Answer.json)
pins these edges, including the Object constructor's signed name:

```text
PACKAGE
  TERMREFpkg example
  TYPEDEF Answer
    TEMPLATE
      APPLY
        SELECTin <init>[SIGNED: () -> java.lang.Object]
          NEW
            TYPEREF Object (TERMREFpkg java.lang)
          SHAREDtype -> the same Object type
      DEFDEF <init>
        EMPTYCLAUSE
        TYPEREF Unit (TERMREFpkg scala)
        STABLE                     // modifier, no constructor RHS
      DEFDEF value                 // no EMPTYCLAUSE
        IDENTtpt Int
          TYPEREF Int (TERMREFpkg scala)
        INTconst 42
    ANNOTATION scala.annotation.internal.SourceFile("<source>")
```

The full files contain `ASTs`, `Positions`, an empty `Comments` section, and
`Attributes` with `SOURCEFILEattr`. The SourceFile annotation, Object parent
construction and default constructor are compiler-generated; class/method names,
parameter clauses and the method RHS derive from the fixture source. The wire
constructor return tree is Unit; its effective semantic result is the class
instance type, as explained in the design and unpickler documentation.

**Resolved metadata decision:** the independent Scala 3.9.0 compiler also accepts
Answer after removing the trailing compiler-injected SourceFile annotation and
the Positions/Comments sections. The reduced file retains `ASTs`, `Attributes`
with `SOURCEFILEattr`, and the exact parent/constructor/method/RHS structure above.
The first producer may therefore omit those optional sections and that compiler
provenance annotation. Keep the observed `IDENTtpt` result wrapper, constructor
STABLE marker and other declaration/reference edges for the first slice; this
experiment does not authorize arbitrary simplification.

The `reduce-answer` control modifies an **existing Scala pickle**, not a typed
program. It accepts only the bounded Answer shape with the expected provenance
annotation, checks retained node addresses do not move, and validates the result.
It retains the original name table and header UUID, with some now-unused names.
It is not a fresh-label allocator, general AST editor, or proof of semantic
pickling. Fresh Rust-generated output still requires #1194/#1195 validation.

## Snapshots and comparison rules

The small Rust tool [`tools/tasty-pickler-oracle/`](../tools/tasty-pickler-oracle)
depends only on the existing `dotty-tasty` codec and the already-used JSON crate.
Its `inspect` command first calls the Scala-3.9 wire validator, then emits:

- version/tooling, decoded names (including qualified and signed names), actual
  section inventory and source-file attribute presence;
- every visible node, parent/child edges, nearest enclosing declaration/package
  owner, ordered method header/parameter names, RHS presence and modifier tails;
- symbolic AST addresses (`n0`, `n1`, ... in node order), reference target labels
  and direction, literal values, and node associations for source positions.

Owner fields express **structural wire nesting**, not an independent semantic
symbol table. Declaration labels distinguish same-spelled symbols in different
owners; direct/reference edges never resolve by searching rendered names.
The checked sources contain no forward **address** references: source-order
forward method selection is named. Direction remains an explicit reference field
so future oracle fixtures can expose an actual forward address when needed.

Normalization excludes UUID bytes, numeric name-table indexes, byte offsets,
source path spelling/coordinate values and section/name-table enumeration order.
Only the name reference identified by SOURCEFILEattr is normalized to `<source>`;
arbitrary string literals are preserved. Composite names resolve their actual
referents before sorting, including signed result/parameter names. AST topology,
target identities, type/name namespaces conveyed by tags, declaration/parameter
order, clause distinctions, modifiers and actual literal values remain checked.
`inspect ... --raw-addresses` additionally prints original AST byte addresses.

The full golden snapshots are structural regressions over **independently
generated** compiler files. The inspector is our decoder, so those snapshots
alone cannot prove semantic parity or independent interoperability. The tests
also make focused assertions about the exact owner, second-parameter reference,
constructor clauses/result syntax, return-type wrapper and actual literal RHS.

| Check | Proven claim |
| --- | --- |
| Rust parser/validator | Target version and valid wire/reference structure within codec checks |
| Snapshot / focused AST assertions | Exact observed structure and identities for these fixtures |
| Semantic unpickler parity (future producer gate) | Normalized source-vs-decoded declaration/owner/type relationships; not typed-body reconstruction |
| Independent decompiler | Scala reader can load/print the candidate; not full compilation |
| Independent `-from-tasty` plus new output class | Scala compiler phases accepted this candidate, without an original class masking it |
| Future #1195 runtime test | The class compiled from actual Rust output returns 42 |

No encoder/decoder agreement or byte-for-byte fixture roundtrip substitutes for
the independent compiler gate. Raw fixture SHA-256 values authenticate the checked-in
inputs; regenerated output comparison uses normalized structure rather than
requiring Scala to reproduce incidental bytes.

## Executable offline workflows

From the repository root, configure explicitly cached tools:

```sh
export SCALA39_ROOT=/path/to/scala3-checkout-at-the-pinned-revision
export JAVA_HOME=/path/to/temurin-21.0.8
export COURSIER_MAVEN_ROOT=/path/to/cached/maven2
python3 tools/tasty-pickler-oracle/run.py check
```

`COURSIER_MAVEN_ROOT` contains the usual group/artifact/version/JAR layout:
compiler, scala3-library, scala-library, tasty-core and scala3-interfaces **3.9.0**;
scala-asm **9.9.0-scala-1**; compiler-interface **1.12.0**; util-interface
**1.11.5**. Exact filenames and hashes are in provenance.json. The tool builds
the Rust inspector with `cargo build --locked`; `CARGO_TARGET_DIR` is honored.
`TASTY_PICKLER_INSPECTOR=/absolute/path/to/tasty-pickler-oracle` can select an
already-built inspector. Temporary files use Python's standard TMPDIR setting.

`check` checks fixture provenance, recompiles sources into a fresh temporary
directory and compares all five normalized snapshots; independently decompiles
and compiles each checked-in standalone fixture and reduced Answer; and requires
Rust/Scala rejection of invalid magic and Scala rejection of a deliberately
misplaced package file. A JSON result records each successful reader/compiler
stage, fresh class and absence of adjacent input classfiles. On 2026-10-16 all
five full fixtures plus reduced Answer passed with the pinned inputs above.

Use the same executable harness for a future producer's candidate:

```sh
python3 tools/tasty-pickler-oracle/run.py verify /path/to/Answer.tasty
```

This first validates/inspects the candidate in Rust, derives its package/class
path and stages **only its TASTy bytes** under a fresh candidate root. A separate
empty output directory must gain a nonempty class file. The bounded inspector
may reject valid Scala outside the characterized subset; that is a harness
limitation, not a claim that such Scala is invalid.

For direct reproduction, the essential input layout and commands are:

```text
candidate/example/Answer.tasty   # no input .class or source
compiled/                       # initially empty, distinct from candidate/
```

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.decompiler.Main \
  -classpath "$SCALA39_CLASSPATH" candidate/example/Answer.tasty
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -from-tasty -d compiled \
  candidate/example/Answer.tasty
```

Here SCALA39_CLASSPATH is the path-separated list of the eight pinned JARs.
The harness constructs it from COURSIER_MAVEN_ROOT and excludes original compiler
output/source directories. Scala derives the classpath root from the declared
package path; `candidate/Answer.tasty` for package example is rejected. No adjacent
classfile/TASTY attribute is needed for this verified standalone layout. JAR input
and arbitrary renamed layouts are outside this harness's verified contract.
Pass `-decompile` to the launcher or use the explicit decompiler Main above;
`dotty.tools.dotc.Main -decompile` is not the decompiler entry point.

Regenerate only for an intentional, reviewable fixture change:

```sh
python3 tools/tasty-pickler-oracle/run.py generate --check # compare structure only
python3 tools/tasty-pickler-oracle/run.py generate         # explicitly update new fixtures/provenance
cargo run --locked -p tasty-pickler-oracle -- inspect \
  tests/fixtures/tasty_pickler/tasty/example/Answer.tasty --raw-addresses
cargo test --locked -p tasty-pickler-oracle
```

Ordinary tests use committed fixtures and mocked compiler failure controls;
they do not claim a new independent run occurred. There is no silent skip in the
explicit `check` or `verify` commands: missing configuration/executable/JARs,
wrong revision/version/hash, nonzero compiler exit, empty decompiler output,
missing fresh class or unexpected negative-control acceptance fails the command.
Each subprocess has a 180-second timeout; timeouts or unavailable tools are harness
failures and cannot count as a successful invalid-input rejection. The inspector
limits input to 1 MiB, AST depth to 256, name depth to 128, visible nodes/names to
10,000 each and total name expansion visits to 100,000.
Actual missing-tool CLI rejection was exercised as well as the unit controls.
The final sprint gate must compile **Rust-produced** bytes and execute the emitted
class; this issue's compiler-generated/reduced fixtures cannot satisfy it.
