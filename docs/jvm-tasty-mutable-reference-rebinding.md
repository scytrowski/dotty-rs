# Mutable local reference rebinding: Scala 3.9 oracle

This report records issue #2298's narrow fixture audit. The source and Java
probe are in [`tests/fixtures/jvm_tasty_mutable_reference_rebinding`](../tests/fixtures/jvm_tasty_mutable_reference_rebinding).
The six `.class` and `.tasty` files under `scala39/` are outputs from the pinned
Scala compiler, not Rust artifacts. The adjacent `oracle.json` records hashes,
commands, and observed control results.

The source keeps `Counter` and `Pair` declaration shapes fixed. In
`PairFactory.create`, it allocates two counters, initializes mutable `selected`
from `first`, reassigns it to `second`, calls `selected.set(nextValue)`, then
constructs `Pair(first, second)`.

```scala
def create(firstSeed: Int, secondSeed: Int, nextValue: Int): Pair =
  val first: Counter = new Counter(firstSeed)
  val second: Counter = new Counter(secondSeed)
  var selected: Counter = first
  selected = second
  selected.set(nextValue)
  new Pair(first, second)
```

## Pinned Scala and JVM evidence

The Scala source revision is
`777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0, TASTy 28.9.0),
compiled with sbt 1.12.1 and Eclipse Temurin JDK 21.0.13+9. The compiler emits
class-file major version 65 and the `create` descriptor
`(III)Lexample/Pair;`. The method allocates exactly two `Counter` instances.
Its local slots are `first=4`, `second=5`, and `selected=6`. Bytecode stores
first and then second into slot 6; the setter loads slot 6 once. Pair
construction loads slots 4 and 5, preserving both original references.

The PairFactory TASTy block begins at address 65. Its statements are `first`
at 83, `second` at 106, mutable `selected` at 127, assignment at 137, and the
setter call at 144; the block result constructs Pair at 67. The assignment
left side is a direct reference to `selected` at 127. Its right side is a
shared reference resolving to the `second` local at 106. The initializer for
`selected` resolves to `first` at 83. The final constructor arguments resolve
to `first` and `second` in that order.

The Java 21 probe runs under `java -Xverify:all -ea`. It verifies that all
three calls preserve distinct Pair fields and produce `(left,right)` values
`(7,42)`, `(-9,11)`, and `(5,-13)`. Equal seeds still produce distinct
objects, so value equality cannot substitute for reference identity.

## Rust source identity and current producer boundary

The fixture passes through the real contextual scanner, parser, namer, and
source typer. The regression follows canonical symbols through source-to-typed
bindings: the two constructor arguments refer to the respective seed formals;
the mutable local's initializer refers to `first`; the typed assignment maps
`selected` to `second`; the setter receiver maps back to `selected` and its
argument to `nextValue`; the Pair call still receives `first` and `second`.
This is identity evidence from the frontend; it does not claim a Rust class or
TASTy batch was emitted.

The paired producer currently rejects at its first cross-class constructor
call identity proof with
`SourceShape("candidate method must have one plain formal clause and a concrete public body")`.
The planning callback is not reached, no artifact batch is returned, and no
partial artifacts are published. This issue records that boundary rather than
changing production planner, encoder, ABI validation, or publishing code.

The ownership boundary is explicit: the frontend owns canonical local and
assignment identities; `dotty-jvm` already owns lowering initialized mutable
reference assignments within its checked unit; the paired TASTy planner and
publisher own any future proof that relates this local-rebinding method to a
cross-class artifact batch. This evidence-only increment changes none of those
production layers. A follow-up that accepts this factory shape belongs at the
paired planner/contract boundary and must preserve the frontend identities
recorded here.

## Controls

The pinned compiler accepts renamed locals, reversed declarations, assignment
back to `first`, duplicate assignment, allocation on the assignment right
side, nested conditional assignment, and nested loop assignment. These valid
shapes remain outside the current paired producer proof and encounter the same
first Rust rejection. The compiler rejects an immutable write (E052), unknown
assignment target (E119), forward reference (E039), uninitialized local under
`-Werror`, and assignment of `Int` to `Counter` (E007). The Rust regression
keeps valid-but-deferred and invalid cases distinct. Exact controls and their
pinned outcomes are summarized in `oracle.json`.

## Initial toolchain blocker

The first pinned compiler launch failed before Scala compilation because sbt's
default Unix-domain socket path exceeded the platform socket path limit under
the long managed scratch directory. Setting sbt's supported
`XDG_RUNTIME_DIR` to a short alias located inside that same registered scratch
resolved the path-length failure. The sandbox then reported `EPERM` while
binding the Unix socket; the same bounded build command completed under the
host-approved execution. The Scala source revision, compiler dependencies,
JDK 21, and Rust 1.98.1 pins were unchanged. This host setup issue is resolved;
the strict test rebuilds the checked-in oracle from the pinned checkout.
