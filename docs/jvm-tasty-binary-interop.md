# JVM classfile and TASTy binary linkage audit

This document records the audit for [issue #1632](https://github.com/scytrowski/dotty-rs/issues/1632) and the follow-up planning increments. It freezes the observed Scala 3.9.0 contract and the current Rust producer overlap. It does not claim that Rust currently emits a Scala-loadable class/TASTy pair.

## Pinned oracle and reproducibility

The oracle checkout was at Scala revision
`777528f19a58e794c9954a42f433373472ec57f8` (`3.9.0-bin-SNAPSHOT-git-777528f`),
using Temurin JDK `21.0.8+9`. The available compiler was built from that
checkout. The source was compiled unmodified:

```scala
package example

class Counter(start: Int):
  private var current: Int = start

  def get(): Int = current
  def set(value: Int): Unit = current = value
```

Reproduction used a fresh output directory and the pinned compiler:

```sh
SCALA_ROOT=/home/vdrake6/dotty-rs/.worktrees/scala39-source
JAVA_HOME=/home/vdrake6/.jdks/jdk-21
export SCALA_ROOT JAVA_HOME
export PATH="$JAVA_HOME/bin:$SCALA_ROOT/dist/target/universal/stage/bin:$PATH"
export TMPDIR=/dev/shm
export JAVA_TOOL_OPTIONS='-Djava.io.tmpdir=/dev/shm -XX:-UsePerfData'
mkdir -p /dev/shm/issue1632/oracle-out
scalac -d /dev/shm/issue1632/oracle-out tests/fixtures/jvm_tasty_interop/Counter.scala
javap -classpath /dev/shm/issue1632/oracle-out -v -p example.Counter
```

The checked-in oracle pair under
`tests/fixtures/jvm_tasty_interop/scala39/example/` is byte-for-byte that
compiler output:

| Artifact | Size | SHA-256 |
| --- | ---: | --- |
| `Counter.class` | 643 bytes | `56f69ba7e3770084683a63e3b6a0c2b4980fe1ea2b83986e03bbcf082e6da053` |
| `Counter.tasty` | 547 bytes | `64ac99472460d76cca64c3610af5e90be912b3c1bef0087211c09e630302e5f4` |

Compiling the same source into a second fresh directory produced the same
`.tasty` bytes and UUID. Scala's `TastyPickler` computes the two UUID longs from
hashes of the name table, tree section, and other sections; this is deterministic
for identical serialized sections. The class attribute must be copied from the
UUID of the exact `.tasty` payload being staged, never independently generated
or carried forward from an earlier build.

## Observed classfile/TASTy link

The Scala-generated classfile has major version **61**, internal name
`example/Counter`, one field, three methods, and five class-level attributes.
Its constant pool has a UTF8 entry for the case-sensitive name `TASTY`. There is
exactly one class-level `TASTY` attribute. Its complete payload is the 16 UUID
bytes, in big-endian most-significant-long then least-significant-long order:

```text
00 9d 71 1b cc 01 99 e8 00 ce 94 0d d0 72 1c 1b
```

The TASTy header parser reports format **28.9.0** and the same 16 bytes. The
attribute does not embed the TASTy document. Scala's pinned `CodeGen.scala`
reads the TASTy UUID and writes its most-significant bits followed by its
least-significant bits to `TASTY`; its comment explicitly associates a 16-byte
payload with a sidecar `.tasty` file. The header's compiler tooling string is
`3.9.0-bin-SNAPSHOT-git-777528f`.

Observed class-level attributes, in this fixture's order, are:

1. `Signature` (the class signature names `java/lang/Object` as its superclass),
2. `SourceFile` (payload is `Counter.scala`),
3. `TASTY` (the 16 UUID bytes above),
4. `Scala3InlineInfo` (four bytes, `01 00 00 00`),
5. `Scala` (empty marker).

`Scala` is emitted on every Scala classfile by Dotty. The pinned
`ClassfileParser` uses `Scala` without `TASTY` to distinguish a Scala
compilation artifact lacking embedded Scala tree data. `Scala3InlineInfo` is
present in this fixture; this audit does not establish it as mandatory for all
classes. The Rust `dotty-classfile` model currently preserves unknown
class-level attributes losslessly as `Attribute::Other`. Its typed
`TastyLinkId` builder/accessor contract writes or validates only the audited
16-byte class-level link; it does not inspect TASTy contents or establish that
the link matches a sidecar.

The sidecar belongs next to the class at the same classpath package path:
`example/Counter.class` and `example/Counter.tasty`. Scala's TASTy loader checks
the sibling classfile's UUID when one exists. A standalone TASTy output without
a sibling classfile is supported for early/output-only TASTy workflows and
produces an informational “No classfiles found” message instead.

### Observed link validation behavior

- A matching class/sidecar pair is accepted.
- Removing the `.tasty` sidecar while retaining the classfile `TASTY` attribute
  made Scala 3.9 fail compilation with: “It should have been loaded from
  `.tasty` file. This `.tasty` file is missing.”
- Changing one sidecar UUID byte while retaining the classfile succeeds by
  default, prints an “out of sync” warning, and says it loaded the TASTy file.
  With `-Werror` the same command exits nonzero because of that warning. UUID
  mismatch is therefore **not a default hard failure** in this pinned compiler.
- The pinned `ClassfileTastyUUIDParser` reports an error if a `TASTY` attribute
  length is not exactly 16. The old embedded-TASTy payload shape is explicitly
  unsupported. This source inspection is the basis for the rule; malformed
  length was not run as an additional compiler control.
- For `-from-tasty`, `ReadTasty` rejects a classfile without `TASTY`, even if
  it has Java-shaped public members. This is distinct from ordinary Scala
  classpath loading and from compiling a fresh class from TASTy.

The classfile's major version is not the TASTy version and does not change the
link payload. Scala 3.9 emitted major 61 by default. For a separate consumer
control, the major bytes of this fixture were changed from `00 3d` (61) to
`00 41` (65), keeping both files otherwise unchanged. The pinned Scala 3.9
compiler successfully compiled the separate client below against that major-65
pair on JDK 21. This establishes reader acceptance for this control only; it
does not establish that an unmodified Rust class passes runtime verification.

## Counter source, TASTy, and JVM ABI overlap

The Scala-generated ABI for this source is:

| Source declaration | Rust TASTy source shape | Rust JVM class | Scala 3.9 baseline class |
| --- | --- | --- | --- |
| `Counter(start: Int)` | class and primary constructor; separate template/constructor parameter identities for `start: scala.Int` | public `<init>(I)V` | public `<init>(I)V`; `MethodParameters` records `start` |
| `private var current: Int` | class-owned private, mutable `VALDEF` of `scala.Int`, initialized from `start` | private field `current:I`; no public current getter/setter | private field `current:I`; no public current getter/setter |
| `get(): Int` | public source `DEFDEF`, no parameters, result `scala.Int` | public `get()I`, bytecode reads the private field | public `get()I`, bytecode reads the private field |
| `set(value: Int): Unit` | public source `DEFDEF`, parameter `value: scala.Int`, result `scala.Unit` | public `set(I)V`, bytecode writes the private field | public `set(I)V`, bytecode writes the private field; `MethodParameters` records `value` |

For all three classfile outputs (Scala source, Rust TASTy recompiled by Scala,
and Rust JVM), the exact member order is constructor `(I)V`, `get()I`,
`set(I)V`; the field order is the single `current:I`. The Rust JVM class uses
major 65, while the Scala baseline and `-from-tasty` class use major 61. The
Rust class currently has no `TASTY` link attribute, so the source/TASTy/ABI
agreement is characterized independently rather than as a loadable pair.

In the source tree/pickled shape, `Unit` is the Scala result type; in the
post-Getters JVM method descriptor it is `V`. The private source var stays
private storage in this case: it is not an accessor pair. This distinction is
important for future public constructor `val`/`var` support, where TASTy
declaration shape and post-Getters ABI can contain generated accessors that do
not correspond to explicit source `DefDef` trees.

The exact same checked-in source now has independent characterization coverage
through the current Rust pipelines:

- `exact_jvm_tasty_linkage_counter_source_is_accepted_by_pickler` builds two
  pickler results, asserts deterministic bytes, and parses them as TASTy 28.9.
  With `DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE=1`, it also stages only the
  Rust-produced `.tasty` and runs the pinned compiler's `-from-tasty` mode into
  a separate empty output directory; the test decodes that fresh class and
  asserts its private field, exact constructor/method order, descriptors, and
  public method access flags. No input classfile is staged.
- `exact_tasty_linkage_counter_source_is_accepted_by_jvm_backend` sends that
  source through the actual source/JVM test adapter and confirms class
  `example/Counter`, major 65, private `current:I`, public constructor `(I)V`,
  public `get()I`, and public `set(I)V` in that order. It also confirms the
  emitted class does not yet have a `TASTY` link.
- `scala_390_counter_classfile_links_to_its_tasty_header` decodes the pinned
  Scala fixture with the Rust classfile and TASTy decoders and checks exact
  class path/name, field/method descriptors, class attributes, UUID equality,
  and the pinned UUID bytes.

### One-session paired planning (#1635)

The root `dotty::jvm_tasty_plan::with_paired_jvm_tasty_class_plan` coordinator
now runs the source frontend once for the audited one-class shape, then builds
both a `JvmCompilationUnit` and a complete `ValidatedClassStatePlan` from the
same `SemanticStore`, source AST/index, typed AST/index, class, constructor,
field, methods and typed RHS roots. It invokes the #1634 consistency proof
before calling the consumer closure. The `PairedJvmTastyClassPlan` exposes the
validated producer plans, compatibility proof, canonical declaration IDs,
source member order, and intended relative `.class`/`.tasty` paths.

The receipt is callback-scoped because these producer plans borrow one session;
the callback cannot retain arena IDs after that session is dropped. The
coordinator rejects multi-class units and any source shape rejected by either
producer or the consistency proof. It is a narrow library planning boundary:
it emits no bytes, writes no files, and does not provide a compiler driver or
CLI. Artifact construction and Scala consumer proof remain assigned to later
issues.

The independent producer tests show that each producer accepts the source
shape on its own. The paired-planning regression additionally asserts shared
session identity, source member order, constructor formal identity, exact
field/method typed roots, and package paths before returning a plan. No class
or TASTy bytes are emitted by that check, so it does not establish byte-level
linkage or Scala consumer behavior.

## Consumer proof and controls for the successor gate

The exact Java-shaped consumer used for the oracle control is:

```scala
package consumer
object Consumer:
  def update(counter: example.Counter): Int =
    counter.set(value = 41)
    counter.get()
  def make(): example.Counter = new example.Counter(start = 23)
```

With only the classfile/TASTy pair in `example/`, Scala 3.9 compiled this
separate client successfully. The paired files were on the compile classpath;
the source `Counter.scala` and any freshly regenerated `Counter.class` were not.
The classfile-only control (same classfile, no sidecar) failed with the missing
`.tasty` diagnostic above. This is a useful discovery control, but the client
only calls public JVM-shaped methods and alone is not conclusive proof that
type information came from TASTy.

The successful compile command was equivalent to:

```sh
scalac -classpath /dev/shm/issue1632/linked \
  -d /dev/shm/issue1632/consumer-out /dev/shm/issue1632/Consumer.scala
```

The classfile-only control used a separate classpath root containing only
`example/Counter.class` and failed with status 1. For the version-65 control,
only the classfile header's major bytes were changed from 61 to 65 before the
same client compile; that compile succeeded. The checked-in Rust characterization
test independently pins the original Scala fixture, so these mutated controls
remain ephemeral and are not mistaken for Scala-generated baseline bytes.

For a positive TASTy-consumption proof, the successor consumer test should add
a deliberate **compile-only control built from Rust output**: start with the
Rust-linked `Counter.class` + the matching Rust-produced `Counter.tasty`, then
remove `get` from a copy of the classfile while preserving its `TASTY` payload
and sidecar. A separate Scala client that calls `get()` must still compile
against this control, because Rust TASTy declares `get`; the same client must
fail against a classfile-only control with the sidecar/link removed. This
proves Scala read the Rust-produced TASTy rather than resolving only the JVM
member table. Do not run the intentionally inconsistent classfile at runtime.
Separately compile the ordinary `Consumer` against the real, matched Rust pair
and execute it using only Rust's unchanged `Counter.class`; the source, Scala
oracle classes, and `-from-tasty` regenerated classes must be absent from both
client and runtime classpaths. The matched run checks ABI, while the controlled
Rust-TASTy copy proves semantic consumption.

Keep these workflows distinct:

1. `scalac -from-tasty` compiling a fresh class from Rust TASTy proves the
   producer TASTy is readable and lowerable.
2. A separate Scala client compiling against Rust's *linked* `.class` + `.tasty`
   and running against the unchanged Rust class proves linked binary
   consumption and runtime ABI.

Success in (1) cannot stand in for (2). The Scala-generated pair above is an
oracle control, not a Rust-produced pair.

## Negative matrix to preserve for follow-up issues

The audit records observed Scala behavior separately from requirements for the
future Rust publisher. Failures should be detected in preflight before either
artifact becomes visible.

| Case | Observed Scala 3.9 behavior / required first rejection layer |
| --- | --- |
| Missing sidecar for class with `TASTY` | Scala classfile/TASTy discovery reports missing `.tasty`; publisher preflight must reject an incomplete pair. |
| Class has no `TASTY`, explicit `-from-tasty` | Scala rejects it in `ReadTasty`; future linked publisher must reject before writing. |
| UUID mismatch | Scala warns and still loads sidecar; `-Werror` fails. Rust preflight should reject mismatch as a pair-integrity error, before staging. |
| `TASTY` payload length other than 16 | Pinned UUID parser reports an error; `ClassFile::tasty_link_id()` rejects it with `InvalidPayloadLength`. |
| Wrong TASTy format version | Rust TASTy 3.9 validation rejects at TASTy validation; publisher must not attach a UUID from an unsupported payload. |
| Package path / internal class name mismatch | Not characterized as a Scala diagnostic here. Shared-output validation must compare the canonical package path and internal name before encoding/publication. |
| Source method, constructor, field, descriptor, or visibility disagreement | Must be rejected by the source/JVM/TASTy cross-artifact validator before either artifact is published; do not infer equivalence from matching names. |
| Scala-private source field represented as public JVM accessor | The fixture has no generated accessor. The future validator must model source-vs-post-Getters rules explicitly and reject an unproven mismatch. |
| Duplicate class-level `TASTY` attributes | Not characterized as a Scala rejection. `ClassFile::tasty_link_id()` and `ClassFileBuilder::tasty_link()` reject duplicates to avoid ambiguous readers. |
| Same simple class name in distinct packages | Use package-qualified identity and sidecar paths; include a two-package regression test before supporting batch output. |
| Unsupported producer shape | Reject during plan validation before writing either artifact; do not emit just the class or just TASTy. |

An ordinary classpath compile that appears to accept an artifact is not a
replacement for these negative assertions: Scala may warn on a stale UUID, and
classfile-only JVM linkage does not establish source/TASTy parity.

## Same-session semantic pair proof (#1634)

`dotty-jvm-tasty-contract` is the acyclic coordination layer above the
independent JVM and TASTy producers. Its
`validate_jvm_tasty_class(&JvmCompilationUnit,
&ValidatedClassStatePlan)` accepts only already validated inputs and returns a
borrowed `ValidatedJvmTastyClass`. The proof retains both input borrows, emits
no bytes, and is limited to the class-state subset represented by the
`Counter` fixture.

Before returning a proof it checks:

- both plans borrow the exact same `SemanticStore` and typed AST; source class,
  constructor, fields, methods, and formals retain the same canonical IDs and
  source ownership;
- the primary constructor ID and ordered formal IDs match, with primitive
  source formals projected to the checked JVM descriptor and the JVM
  constructor result fixed to `V`;
- class-state field and ordinary method identities occur in the same explicit
  source order in the JVM plan, with no missing producer member;
- every field initializer and concrete method RHS uses the exact typed root
  associated with its source tree, and the TASTy binding index maps that source
  tree to the same root;
- the audited private mutable primitive field remains private, non-final
  storage without a generated public accessor, and method descriptors are
  independently projected from the canonical source method types (`Unit`
  maps to `V`);
- the Scala 3.9 encoded relative `.tasty` path equals the JVM internal class
  name after removing `.tasty`. Package segments are retained, so equal simple
  names in different packages remain distinct.

The pairing crate depends on both producer crates; neither producer depends on
the other. `dotty-jvm` remains dependent only on `dotty-core` and
`dotty-classfile`, and `dotty-tasty-pickler` remains dependent only on
`dotty-core` and `dotty-tasty` in production. No typer pass is rerun, and no
symbol, type, or tree identity is created or reconstructed from text.

Focused tests use the exact `Counter` source in one real frontend session and
cover distinct sessions with numerically equal arena IDs, a mismatched typed
initializer root, a missing JVM method, immutable-vs-mutable field shape, and
same-named classes under separate packages. Existing plan validators continue
to reject unsupported source forms before the pair proof is attempted.

The separate `validate_classfile_tasty_link(&ClassFile, tasty_bytes)` helper
checks that a decoded class has one valid `TASTY` link and that its UUID equals
the UUID in a Scala 3.9-validated TASTy header. This rejects missing, malformed,
and stale links before publication. It does not authenticate that the supplied
bytes were produced by the borrowed `ValidatedClassStatePlan`, or that the
classfile bytes were emitted from the paired JVM plan; #1637 must connect those
exact producer outputs before staging them.

Likewise, an arbitrary `SourceTypedBindings` implementation is producer
evidence, not an identity authenticator. The coordinator compares its result
with both plans, while the session owner remains responsible for using the
actual `SourceTypedIndex` from that session.

## Successor boundary

Audit #1632 changed tests, a pinned oracle fixture, and this document only.
Issue #1633 adds the typed, Scala-independent classfile link representation
and validation; it does not change the JVM backend, TASTy format/pickler,
Typer, production dependencies, or add a compiler driver. Issue #1634 now
provides source/JVM/TASTy semantic consistency validation. The remaining work
follows the dependency sequence in #1635–#1639: derive both producer plans from
one source session; atomically stage linked artifacts; prove independent Scala
consumption; then run the full determinism/runtime gate.
