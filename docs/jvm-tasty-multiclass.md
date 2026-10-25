# Scala 3.9.0 two-class JVM/TASTy layout audit

This document records the bounded audit for [issue #1652](https://github.com/scytrowski/dotty-rs/issues/1652) and the subsequent planning, validation, and in-memory emission steps. It freezes the observed output for **one source file containing two independent top-level ordinary classes**. The oracle does not establish a general Scala packaging rule beyond this fixture.

The exact producer input is [`CounterGauge.scala`](../tests/fixtures/jvm_tasty_multiclass/CounterGauge.scala), SHA-256 `e560545eeb46fad4cb6e41a0a86e4b6932bbf221856780ade6fd4492a694f60c`. Scala 3.9.0 was checked out at revision [`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8). The compiler reported `3.9.0-bin-SNAPSHOT-git-777528f`, built from that checkout; the staged `scala3-compiler_3` jar SHA-256 was `b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`. The compiler ran on Temurin JDK `21.0.8+9`. Its default class-file version is 61; the Rust JVM backend's current target, major 65, remains an independent setting.

## Observed output and link map

Compiling the literal source into a fresh empty directory produced exactly these four files:

| Relative path | Bytes | SHA-256 |
| --- | ---: | --- |
| `example/Counter.class` | 648 | `a650908f42813963ee324f8a3648c948ab59bd74d4f31217981d93f460f2bf29` |
| `example/Counter.tasty` | 576 | `da6e9366a104db48e88100d065c9f6f37cd069dd3a44850bc7a3d346239cdf29` |
| `example/Gauge.class` | 644 | `6625be15a2d9fc53a6663144f87a5fc97c816e9a99dab9aad1c9de4b6926f7eb` |
| `example/Gauge.tasty` | 573 | `f9da39f65f194e09b85abd4401ee10829ef0958edc773c9497e60f77758ad88e` |

There is one TASTy sidecar per class in this fixture, not one shared sidecar. Each classfile has one 16-byte class-level `TASTY` attribute, and that payload exactly matches the header UUID of its sibling sidecar:

| Classfile | Sibling TASTy | UUID in both link and header |
| --- | --- | --- |
| `example/Counter.class` | `example/Counter.tasty` | `0075154beaa4bae8004b8b7090f4341b` |
| `example/Gauge.class` | `example/Gauge.tasty` | `005b42ac860bfbe8005a27b4f8edbd1b` |

The two measured UUIDs differ. This is evidence for these exact serialized classes; it is not a global UUID uniqueness contract. The link bytes identify the matching payload, so a producer must derive each class link from the TASTy bytes it will publish. No `CounterGauge.class` or shared `CounterGauge.tasty` was emitted.

Both sidecars parse and validate as TASTy `28.9.0`. Each contains an `ASTs` section with one package root for `example` and one direct class `TypeDef`: `Counter` in the Counter sidecar, `Gauge` in the Gauge sidecar. The classfile `SourceFile` attribute names the basename `CounterGauge.scala`; the TASTy `Attributes` section records the compiler input path `tests/fixtures/jvm_tasty_multiclass/CounterGauge.scala`. The checked-in class and TASTy files under [`scala39/example`](../tests/fixtures/jvm_tasty_multiclass/scala39/example) are these exact compiler outputs. The focused Rust audit test decodes both classfiles, parses and validates both TASTy files, checks each package root and class name, and verifies each class-to-header link.

Scala's compiler writes package-relative artifact paths. The output path set is the contract observed here; filesystem creation order is not used as a semantic ordering guarantee. The Rust batch manifest should preserve source declaration order (`Counter`, then `Gauge`) and reject duplicate final paths before publication. It should associate each canonical class with its own `.class`/`.tasty` path and UUID, rather than deriving identity from simple names alone.

## Bounded class ABI

| Class | Classfile version | Field | Methods, in emitted order | Constructor initialization |
| --- | --- | --- | --- | --- |
| `example/Counter` | 61.0 | `private current:I` | public `<init>(I)V`, `get()I`, `set(I)V` | Calls `Object.<init>()V`, then stores slot 1 into `current`. |
| `example/Gauge` | 61.0 | `private reading:I` | public `<init>(I)V`, `get()I`, `set(I)V` | Calls `Object.<init>()V`, then stores slot 1 into `reading`. |

The mutable backing fields are not final. `get` reads the class's own field; `set` writes that same field and returns `void`. Both constructors and setter parameters have `MethodParameters` names (`start` and `value`). `javap -v -p -c` shows five class attributes on each class: `Signature`, `SourceFile`, `TASTY`, `Scala3InlineInfo`, and `Scala`. In both outputs `Scala3InlineInfo` is `01 00 00 00`; no general requirement for that attribute is inferred from this audit.

A second compile of the unchanged source into another fresh directory produced byte-identical contents for all four artifacts. The Rust classfile decoder and TASTy parser assert the measured member descriptors, access flags, class version, UUIDs, format version, package roots, and source class names against the checked-in oracle files.

## External Scala consumer and failure controls

[`Consumer.scala`](../tests/fixtures/jvm_tasty_multiclass/Consumer.scala) is a separate client. It uses named constructor arguments (`start`) and named setter arguments (`value`), verifies both initial values, mutates the classes independently, checks fresh instances retain their own state, and prints `1329`.

The pinned compiler successfully compiled the client against the two oracle pairs. Running the consumer on JDK 21 with `java -Xverify:all` and the two original classfiles printed `1329`. The Scala consumer output contained only `consumer/Consumer.class`, `consumer/Consumer$.class`, and its own `consumer/Consumer.tasty`; it did not regenerate either producer class. The same consumer also compiled successfully with only the two `.tasty` files on its compile classpath, which confirms Scala can use the sidecars for source semantics without sibling classfiles. Runtime still requires the classfiles.

| Control | Result with pinned Scala 3.9.0 | First observed stage |
| --- | --- | --- |
| Remove `Counter.tasty`, retain both linked classfiles | Compilation fails: “It should have been loaded from `.tasty` file. This `.tasty` file is missing.” | Scala classpath loading; there is no Java-shaped fallback for this linked Scala class. |
| Flip one byte of Counter's classfile `TASTY` UUID, keep its sidecar | Default compilation succeeds with an “out of sync” warning and reports that it loaded the TASTy file. With `-Werror`, compilation fails because the warning is fatal. | Scala classpath loading / diagnostics; mismatch does not silently choose classfile semantics. |
| Move `Counter.tasty` outside `example/` while leaving `Counter.class` in place | Compilation fails with the same missing-sidecar diagnostic. | Relative TASTy path resolution. |
| Move `Gauge.class` outside `example/`, keep `example/Gauge.tasty` | Scala compilation succeeds by loading the TASTy sidecar; executing the client with that classpath fails with `NoClassDefFoundError: example/Gauge`. | Compile-time TASTy loading succeeds; JVM runtime class lookup fails. |
| Provide only both package-relative `.tasty` files | Scala client compilation succeeds; runtime cannot execute without classfiles. | TASTy source-semantic loading. |

These controls distinguish TASTy-based Scala compilation from JVM class availability. A successful client compile alone does not prove that runtime resolves the intended classfile; the consumer gate must also verify and execute the original Rust-produced classfiles.

## Rust frontend and current producer boundary

The regression `counter_and_gauge_keep_distinct_canonical_symbols_in_one_typer_session` runs the literal two-class source through one scanner, parser, namer, semantic store, source index, and `SourceTyper`. In source order it completes both canonical class and constructor identities, the constructor-owned `start` formals, the class-owned fields, and each class's `get`/`set` methods. It types every field initializer and method body and checks the original source-to-typed root mapping. The two classes have distinct class, constructor, formal, field, and method `SymbolId`s; each member owner remains its declaring class even though both classes spell their methods `get` and `set`.

The existing `with_paired_jvm_tasty_class_plan` and `emit_paired_jvm_tasty_class` remain one-class-only, preserving their existing error contract and output path. The regression `multi_class_source_is_rejected_without_a_partial_pair` pins the one-class planner's error for the Counter/Gauge fixture: `UnsupportedSourceShape("exactly one top-level class; multi-class source units are not supported")`. The separate batch planner and emitter are the bounded multi-class path.

`ClassStatePicklePlan` and its validator describe one canonical class tree, constructor, and ordered member slice borrowing the caller's semantic stores and source/typed arenas. `with_paired_jvm_tasty_batch_plan` collects the complete package declaration list, requires 2–32 ordinary classes, and plans them through one scanner/parser/namer/typer session. Its callback-scoped batch receipt contains one complete ordered `JvmCompilationUnit`, one class-scoped TASTy plan and cross-ABI proof per class, and each class's relative `.class`/`.tasty` path. Every proof is checked against that same complete JVM unit. Before whole-unit validation, each class's JVM metadata is validated in isolation so a class-specific failure is reported against the class and source declaration that own it. Remaining complete-unit failures use a batch-level error rather than being attributed to whichever class proof runs first. The planner rejects unsupported siblings, class/member shape errors, duplicate paths, and any failed class without returning a partial batch.

## Owned in-memory batch emission (#1655)

`dotty::jvm_tasty_artifacts::emit_paired_jvm_tasty_batch` consumes the complete
borrowed batch within one `with_paired_jvm_tasty_batch_plan` callback. It first
retains the #1654 whole-source receipt, then serializes each class's real
validated TASTy plan, derives that class's link UUID from its own parsed
28.9.0 header, and emits a classfile from the original full-session JVM unit.
No class is replanned from a synthetic source file. The existing one-class
encoding and validation logic is shared by both entry points.

The API returns `PairedJvmTastyBatchArtifacts` only after every ordered pair
has been encoded and the complete owned classfile/sidecar maps have passed
`validate_jvm_tasty_batch_artifacts`. The return value contains the common
`SourceId` and source-ordered class receipts with canonical class `SymbolId`,
member order, package-relative paths, complete owned byte vectors, and the
producer-derived UUID. If the second class or any cross-artifact check fails,
the function returns an error with class index and symbol where applicable;
there is no partial success value. Fresh-session regression tests compare the
ordered paths, every classfile and TASTy byte, and each UUID.

This is **in-memory emission only**. It does not stage or publish files and
does not establish multi-class Scala consumer compatibility. The implementation
is limited to the two-class independent Counter/Gauge fixture shape and the
existing validated source/type subset. It does not change the legacy
`emit_paired_jvm_tasty_class` API or its bytes. Filesystem publication belongs
to #1656, and the independent pinned Scala 3.9.0 consumer proof for both classes
belongs to #1657.

## Test-only complete batch publication (#1656)

The `jvm_tasty_artifacts.rs` integration-test adapter accepts the complete
owned batch from #1655. It re-decodes every classfile and TASTy document,
checks class-file and TASTy versions, canonical class names and paths, exact
class/sidecar mapping, class-to-header UUID links, common source identity,
distinct class identities and output paths, and the total byte limit before it
creates a staging directory. The adapter currently permits 2–32 class pairs
and at most 64 MiB of artifact bytes. For #1652's audited Counter/Gauge map,
each class has its own sidecar; every required classfile and sidecar is
inventoried once in deterministic source order.

The destination must be an absent path beneath an existing, non-symlinked
directory. Existing destinations are rejected without replacing or deleting
their contents. After prevalidation, the adapter creates a unique sibling
staging directory (mode `0700` on Unix), writes each file with exclusive
creation, flushes it, then checks the staged tree inventory and every byte
against the manifest. One same-filesystem directory rename promotes the full
tree. A handled staging or promotion error removes the staging tree best
effort; an injected post-promotion failure removes the newly promoted tree
best effort and reports cleanup errors alongside the original error. Fault
injection covers each of the four file writes, pre-promotion validation, and
post-promotion rollback. The existing one-pair publisher and its tests remain
unchanged.

This integration helper is not a production filesystem API. Rollback covers
reported errors while the process is running. It does not provide durability
against power loss or process termination, a transaction across filesystems,
or protection from a concurrent process with the same user permissions racing
the destination or staging tree. The rename is used only on the same filesystem;
no cross-device fallback or crash-atomic transaction is claimed.

## Strict Scala consumer gate (#1657)

`tests/jvm_tasty_multiclass_scala_consumer.rs` is a separate opt-in integration
target. With `DOTTY_REQUIRE_SCALA_TASTY_MULTICLASS_CONSUMER=1`, missing toolchain
configuration or any failed consumer/control check fails the test; without the
variable it reports a skip. It uses the #1656 batch publisher to stage two
independently emitted fresh-session manifests, then hands only those staged
Rust artifacts to the pinned Scala 3.9.0 harness. The older one-class
`DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1` gate and its `consume-pair` mode remain
unchanged.

The harness verifies each batch contains exactly the four expected
package-relative artifacts, compares every classfile and sidecar byte across
fresh sessions, checks classfile major 65 and each classfile-to-TASTy UUID link,
and records the artifact hashes. It compiles the checked-in separate
[`Consumer.scala`](../tests/fixtures/jvm_tasty_multiclass/Consumer.scala) only
against each Rust batch, checks that the client output contains no producer
classes, and checks its bytecode references both constructors, setters, and
getters. Running each result with `java -Xverify:all` must print `1329`; the
Rust artifacts are hashed again afterward to prove compilation and execution
did not change them.

Per class, the harness also removes `get` from a copy of the classfile while
retaining its TASTy file and compiles a dedicated method-use probe. A passing
probe demonstrates that the Scala typer resolved `get` from that class's TASTy
semantics. The corresponding classfile-only copy must fail because `get` is
absent. Separate missing-sidecar and stale-UUID copies must fail with the
expected TASTy diagnostics under `-Werror`. Each control has a fresh output
directory, preserves the sibling class artifacts, and an unexpected failure
layer or diagnostic fails the gate. The classfile-only read-bytecode fallback
is checked for each class by renaming its marker to `XASTY` and by removing the
marker entirely while withholding its sidecar; neither copy may resolve the
deliberately removed `get` method. The missing-classfile controls compile from
the TASTy sidecar and then require runtime `NoClassDefFoundError` for that exact
class. A misplaced sidecar must be reported as missing, and a sidecar with a
damaged TASTy magic must be rejected as unreadable.

The observed first failure for both classes is class-specific: missing class
files fail at JVM loading with `NoClassDefFoundError: example/Counter` or
`example/Gauge`; missing and misplaced sidecars report that the linked Scala
binary should have been loaded from `.tasty` but that file is missing; damaged
sidecars report `not a TASTy file`; stale UUID warnings contain `out of sync
with its TASTy file` and become errors under `-Werror`; and XASTY/missing-marker
probes report `value get is not a member` because their classfile lacks that
method and no valid sidecar is available. These diagnostics are also emitted
in the test's JSON report alongside the class names. The separate consumer
checks a second instance of each class after mutating the first, so constructor
state remains independent.

To reproduce this strict gate from the repository root, use the Scala source
checkout at the pinned revision and the cached Maven artifacts whose SHA-256
provenance is checked by the harness:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/jdk-21-or-newer \
COURSIER_MAVEN_ROOT=/path/to/coursier/https/repo1.maven.org/maven2 \
DOTTY_REQUIRE_SCALA_TASTY_MULTICLASS_CONSUMER=1 \
cargo test -p dotty --test jvm_tasty_multiclass_scala_consumer \
  --locked -- --nocapture
```

This issue's strict consumer run used Scala `3.9.0-bin-SNAPSHOT-git-777528f`
from revision `777528f19a58e794c9954a42f433373472ec57f8` on Corretto JDK
`25.0.4`. The harness prints the JDK release, checked compiler dependency
hashes, fixture hash, per-class artifact hashes and link UUIDs, method
references, control results, and both `1329` runtime outputs as JSON. The
repository's separate mandatory JVM CI gate continues to validate JDK 21 class
file verification and execution. For each clean batch the compiler receives
only `-classpath <Rust batch>:<pinned Scala jars> -d <empty client output>` and
the separate consumer source; execution uses
`java -Xverify:all -classpath <Rust batch>:<client output>:<pinned Scala jars>
consumer.Consumer`.

The strict Rust producer run emitted these identical artifacts in both fresh
sessions (distinct generated UUIDs are shown separately):

| Rust artifact | SHA-256 |
| --- | --- |
| `example/Counter.class` | `c3620a88edbc9759ecaff244f7a9144c36ac70a5128ff9803aa08471a2c4dd81` |
| `example/Counter.tasty` | `f0ad78c019e349b7496556164d02d8c5e5252194b02e39c1cb5cded2f2f1c219` |
| `example/Gauge.class` | `24fb39b52f1342ab15bcaa9bbafab57f8f4a542c142c6750f87aa476d0a47bdc` |
| `example/Gauge.tasty` | `ddd954ac371e08f2c7c6644c8041ea04b99081467cd5e60013f17a7a0b907e98` |

Counter's class/TASTy UUID is `00ce38125fe1b1ac0000000000008193`; Gauge's is
`000dc09f21d551ac0000000000008193`. The compiler reported both class-specific
`1329` runtime outputs. The JSON diagnostics preserve the first compiler or
JVM failure output for each class/control combination, including the missing
classfile `NoClassDefFoundError`, missing/wrong-path sidecar message, invalid
TASTy magic message, stale-link warning promoted by `-Werror`, and absent-get
method errors for classfile-only/XASTY/no-marker controls.

## Successor boundaries

- **#1653** adds one-session whole-unit planning while preserving both independent classes, all member IDs, typed roots, and source order.
- **#1654** validates every class and path together and checks each link against its exact sidecar bytes.
- **#1655** emits the complete owned in-memory artifact set and rejects partial batches.
- **#1656** stages and publishes the complete artifact set in a test-only
  adapter, with handled-error rollback and explicit filesystem limits.
- **#1657** uses the strict Scala consumer gate above to test TASTy source semantics for each class separately, reject classfile-only/missing-sidecar/stale-link controls, and execute the separate client against unchanged Rust classfiles.
- **#1658** is the strict fresh-session deterministic release gate. It must compare all class and TASTy bytes and paths, not only decoded shapes.

Cross-class source calls/fields and inheritance between these siblings, objects/companions, nested classes, traits, generics, overloaded/default methods, external JDK classes, `String`, public constructor accessors, and other unsupported pickler forms remain outside this audit and the bounded successor fixture.

## Whole-batch consistency contract (#1654)

`dotty-jvm-tasty-contract::validate_jvm_tasty_batch` consumes the complete
ordered `JvmCompilationUnit` from #1653 and one validated class-state TASTy
plan for each class (2–32 classes, matching #1653's planner bound). It returns
a read-only receipt only after every class has passed the existing canonical
ABI proof. The receipt preserves source order,
the common `SourceId`, each original class `SymbolId`, and its exact
package-qualified `.class` and `.tasty` paths. Validation checks that both
plans borrow the same store, source AST/index, definitions, typed AST, and
source ID; both caller lists exactly match the ordered class declarations
recovered from the canonical source package; and package ownership,
class names, source member/formal identities, typed RHS bindings, and ABI
descriptors agree; and no class, binary name, path, or original symbol is
represented twice. A failing later class rejects the receipt for the entire
batch.

The separate `validate_jvm_tasty_batch_artifacts` function consumes owned,
independently keyed classfile and TASTy sidecar maps. Input order is immaterial:
it sorts both path sets, rejects duplicate/cross-colliding paths, then checks
the expected pair set in source order. Each pair must use the exact planned
paths, decode without trailing classfile bytes, identify the expected
package-qualified class, target classfile 65.0, match the checked
class/member/access-flag ABI, superclass, and empty interface list, carry a
classfile `TASTY` identifier equal to its sidecar header, and parse as TASTy
28.9.0. The complete sidecar bytes must also equal the bytes produced by that
class's validated TASTy plan. There is one sidecar per class for this audited
fixture. The validator does not require UUIDs to be globally unique; the
classfile-to-sidecar link is checked pairwise against the actual header.
Nothing is published by either API.

The first-error taxonomy is deterministic and contextual:

- `BatchShape` rejects incomplete counts, duplicate class plans, unsupported
  source siblings, and source order or coverage disagreement before returning
  any proofs.
- `BatchClass { class_index, class, error }` wraps the first semantic or
  per-class ABI failure in source declaration order.
- `BatchArtifactPaths { reason, path }` reports the first duplicate, missing,
  orphan, or extra package-relative path after deterministic path sorting.
- `BatchArtifact { class_index, class, phase, reason }` reports decode,
  version, identity, class/member ABI, TASTy, or link failures in source order.

The semantic receipt uses original `SymbolId`s and source/index bindings; equal
names or equal numeric arena slots do not substitute for source ownership.
Numeric IDs alone cannot authenticate a foreign same-slot arena, so callers
must preserve the one-session ownership invariant when constructing both the
JVM unit and TASTy plans. The batch validator checks pointer identity for the
shared semantic stores and arenas it receives, but it is not a cryptographic
session token. The official Scala consumer remains the separate #1657 proof.

## Reproduction

From the repository root, with the pinned Scala source checkout and JDK available:

```sh
SCALA_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
JAVA_HOME=/path/to/temurin-21
SCALA_LIB_ROOT=/path/to/coursier/maven2/org/scala-lang
export PATH="$JAVA_HOME/bin:$SCALA_ROOT/dist/target/universal/stage/bin:$PATH"
export TMPDIR=/dev/shm
export JAVA_TOOL_OPTIONS='-Djava.io.tmpdir=/dev/shm -XX:-UsePerfData'
scalac -version
java -version

ORACLE_OUT=$(mktemp -d)
scalac -d "$ORACLE_OUT" tests/fixtures/jvm_tasty_multiclass/CounterGauge.scala
find "$ORACLE_OUT" -type f -printf '%P\t%s bytes\n' | sort
sha256sum "$ORACLE_OUT"/example/*
javap -classpath "$ORACLE_OUT" -v -p -c example.Counter
javap -classpath "$ORACLE_OUT" -v -p -c example.Gauge

SECOND_OUT=$(mktemp -d)
scalac -d "$SECOND_OUT" tests/fixtures/jvm_tasty_multiclass/CounterGauge.scala
for path in example/Counter.class example/Counter.tasty example/Gauge.class example/Gauge.tasty; do
  cmp "$ORACLE_OUT/$path" "$SECOND_OUT/$path"
done

CONSUMER_OUT=$(mktemp -d)
scalac -classpath "$ORACLE_OUT" -d "$CONSUMER_OUT" \
  tests/fixtures/jvm_tasty_multiclass/Consumer.scala
java -Xverify:all -classpath \
  "$CONSUMER_OUT:$ORACLE_OUT:$SCALA_LIB_ROOT/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar:$SCALA_LIB_ROOT/scala-library/3.9.0/scala-library-3.9.0.jar" \
  consumer.Consumer
```

The final command prints `1329`. The source fixture SHA-256 is
`3d3822120d9ab41a8502c545f1693870ec09d3fcc1908f39d283c074787b3508`. The
following controls use fresh classpath and output directories; failures are
expected for the missing sidecar, stale UUID under `-Werror`, and wrong TASTy
path:

```sh
CONSUMER_SOURCE=tests/fixtures/jvm_tasty_multiclass/Consumer.scala
CONTROL_ROOT=$(mktemp -d)

mkdir -p "$CONTROL_ROOT/missing" "$CONTROL_ROOT/missing-out"
cp -R "$ORACLE_OUT/example" "$CONTROL_ROOT/missing/"
rm "$CONTROL_ROOT/missing/example/Counter.tasty"
scalac -classpath "$CONTROL_ROOT/missing" -d "$CONTROL_ROOT/missing-out" "$CONSUMER_SOURCE"

mkdir -p "$CONTROL_ROOT/corrupt" "$CONTROL_ROOT/corrupt-default" "$CONTROL_ROOT/corrupt-werror"
cp -R "$ORACLE_OUT/example" "$CONTROL_ROOT/corrupt/"
python3 - "$CONTROL_ROOT/corrupt/example/Counter.class" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
data = bytearray(path.read_bytes())
link = bytes.fromhex("0075154beaa4bae8004b8b7090f4341b")
offset = data.find(link)
assert offset >= 0
data[offset] ^= 1
path.write_bytes(data)
PY
scalac -classpath "$CONTROL_ROOT/corrupt" -d "$CONTROL_ROOT/corrupt-default" "$CONSUMER_SOURCE"
scalac -Werror -classpath "$CONTROL_ROOT/corrupt" -d "$CONTROL_ROOT/corrupt-werror" "$CONSUMER_SOURCE"

mkdir -p "$CONTROL_ROOT/wrong-tasty" "$CONTROL_ROOT/wrong-tasty-out"
cp -R "$ORACLE_OUT/example" "$CONTROL_ROOT/wrong-tasty/"
mv "$CONTROL_ROOT/wrong-tasty/example/Counter.tasty" "$CONTROL_ROOT/wrong-tasty/Counter.tasty"
scalac -classpath "$CONTROL_ROOT/wrong-tasty" -d "$CONTROL_ROOT/wrong-tasty-out" "$CONSUMER_SOURCE"

mkdir -p "$CONTROL_ROOT/wrong-class" "$CONTROL_ROOT/wrong-class-out"
cp -R "$ORACLE_OUT/example" "$CONTROL_ROOT/wrong-class/"
mv "$CONTROL_ROOT/wrong-class/example/Gauge.class" "$CONTROL_ROOT/wrong-class/Gauge.class"
scalac -classpath "$CONTROL_ROOT/wrong-class" -d "$CONTROL_ROOT/wrong-class-out" "$CONSUMER_SOURCE"
java -Xverify:all -classpath \
  "$CONSUMER_OUT:$CONTROL_ROOT/wrong-class:$SCALA_LIB_ROOT/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar:$SCALA_LIB_ROOT/scala-library/3.9.0/scala-library-3.9.0.jar" \
  consumer.Consumer

mkdir -p "$CONTROL_ROOT/sidecars-only/example" "$CONTROL_ROOT/sidecars-only-out"
cp "$ORACLE_OUT/example/Counter.tasty" "$ORACLE_OUT/example/Gauge.tasty" \
  "$CONTROL_ROOT/sidecars-only/example/"
scalac -classpath "$CONTROL_ROOT/sidecars-only" -d "$CONTROL_ROOT/sidecars-only-out" "$CONSUMER_SOURCE"
```

Expected outcomes: missing sidecar and wrong TASTy path fail during Scala
classpath loading; UUID mismatch warns and succeeds normally, then fails under
`-Werror`; the misplaced Gauge class still permits compile-time TASTy loading
but Java execution fails with `NoClassDefFoundError`; sidecars alone compile
the client. These controls distinguish TASTy semantic loading from JVM class
lookup. They do not imply a Java fallback path for linked Scala classes.
