# Conditional mutable Counter reference assignment audit

Issue #2345 audits a Unit-valued if whose two arms each assign an existing
Counter reference to one mutable local. This is the paired JVM/TASTy shape
between the immutable conditional-reference selection audited in #2265–#2273
and straight-line mutable reference rebinding audited in #2298–#2311. The
fixture is evidence only; it does not widen producer acceptance.

## Reproduce the official Scala and JDK evidence

The frozen source is
[ConditionalMutableReferenceAssignment.scala](../tests/fixtures/jvm_tasty_conditional_mutable_reference_assignment/ConditionalMutableReferenceAssignment.scala).
Its SHA-256 is
ca118afbee8cc160757baceb4666720d46db871f31d3a72c237993c3181ca4e5. It
contains the existing Counter and Pair family and the one target
PairFactory.create method, in one source session. The independently authored
Java probe is in the same fixture. Complete per-file hashes and commands are
in that fixture's [oracle.json](../tests/fixtures/jvm_tasty_conditional_mutable_reference_assignment/oracle.json).

Use the Scala source checkout at revision
777528f19a58e794c9954a42f433373472ec57f8, Scala 3.9.0, sbt 1.12.1, and
Eclipse Temurin JDK 21.0.13+9. The JDK java executable hash is
9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155.
Build the pinned compiler with sbt --batch 'dist/universal:stage', then from
the fixture directory run:

```sh
scalac -Werror -release 21 -Vprint:typer -color:never -d scala39 ConditionalMutableReferenceAssignment.scala
javac --release 21 -cp scala39 -d <managed-scratch>/probe-classes RuntimeProbe.java
java -Xverify:all -ea -cp <managed-scratch>/probe-classes:scala39 example.RuntimeProbe
javap -v -c -p -classpath scala39 example.PairFactory
scalac -color:never -print-tasty scala39/example/PairFactory.tasty
scalac -color:never -decompile scala39/example/PairFactory.tasty
```

<managed-scratch> means a registered scratch directory for build/test output.
The checked-in scala39 class and TASTy files are official compiler output;
they are not Rust-produced artifacts. Captured output is under oracle/.

## Observed JVM behavior

The official descriptor string is "(IIZI)Lexample/Pair;". javap reports
class-file major 65. The method allocates Counter exactly twice, into slots 5 and 6,
then initializes mutable slot 7 from slot 5. It branches with ifeq 35 at
offset 25. The true arm loads slot 6 and stores it to slot 7; the false arm
loads slot 5 and stores it to slot 7. After the join it loads slot 7 and calls
Counter.set(I)V once. The final Pair constructor receives slots 5 then 6,
so it gets the original distinct objects. The method StackMapTable has an
append frame at offset 35 with the three Counter locals and a same frame at
offset 39.

The Java 21 probe runs both Boolean branches and signed/equal seeds. Its output
is:

```text
conditional mutable reference assignment true=7/42,false=42/3,signed=-9/11,11/42,equal=7/42,42/7 distinct=true
```

It uses reflected Pair.left/Pair.right references to check they stay distinct
and checks that only the selected original object changes. The javap evidence
supplies the exact allocation and setter-call counts, branch stores, receiver
slot, and constructor argument order.

## Observed TASTy 28.9.0 shape

PairFactory.tasty has UUID 00b1e7f0-b631-62da-0009-dc6190e83f29. Its create
body is a BLOCK at address 75. The block source stat order is:

1. VALDEF@93 first
2. VALDEF@116 second
3. mutable VALDEF@137 selected
4. IF@147, with ASSIGN@151 in the then arm and ASSIGN@158 in the else arm
5. APPLY@165 selected.set(nextValue)

The block result is APPLY@77 Pair(first, second). The wire printer places
that result before the statements in its child listing; the semantic block
order above follows the statement children. The IF condition is
TERMREFdirect@149 -> chooseSecond PARAM@53. The then assignment targets
TERMREFdirect@153 -> selected VALDEF@137 and its RHS is
SHAREDtype@156 -> TERMREFdirect@91 -> second VALDEF@116. The else assignment
targets SHAREDtype@160 -> TERMREFdirect@153 -> selected VALDEF@137 and its
RHS is SHAREDtype@163 -> TERMREFdirect@89 -> first VALDEF@93. The setter
receiver shares the selected reference; final Pair arguments point to first
then second. Neither arm is wrapped in TYPED, and no explicit Unit wrapper is
emitted: both branch expressions are the assignments themselves.

The full javap, print-tasty, decompile, and typed-tree captures preserve the
surrounding context for those addresses.

## Rust frontend identity and current producer boundary

tests/jvm_tasty_conditional_mutable_reference_assignment_proof.rs runs the
real ContextualScanner -> parser -> namer -> SourceTyper pipeline on the same
source. It asserts one canonical SourceId, three distinct method-owned local
symbols, the exact source-to-typed edge for the Unit-valued If, both branch
Assign nodes, and each target/RHS by canonical TermRef symbol identity. It
also checks the later setter receiver and original Pair argument order.

The direct emit_paired_jvm_tasty_batch call first rejects with:

```text
Plan(BatchUnit {
  phase: "cross-class constructor-call identity proof",
  reason: SourceShape("four-formal candidate requires two initialized locals, selected IF, one setter, then Pair")
})
```

The producer returns an error before a paired plan is accepted, so it returns
no batch or partial artifacts. The current rejection is measured evidence;
this issue does not implement support for the new shape.

## Control classification

controls/ contains PairFactory variants compiled against the pinned oracle
Counter/Pair outputs. The Rust test prepends the original source's canonical
Counter/Pair declarations before passing each control through the paired
producer.

| Control | Scala 3.9 | Rust paired producer |
| --- | --- | --- |
| Renamed locals | Valid | Deferred at the same cross-class SourceShape rejection |
| Reversed first/second declarations | Valid | Deferred at the same rejection; no slot or declaration-order identity guess |
| Immutable assignment target | Invalid, E052 | Rejected in method body typing (AssignmentTargetImmutable) |
| Unresolved foreign reference | Invalid, E119 | Rejected in method body typing (TermNameNotFound) |
| Forward reference before initialization | Invalid, E039 | Rejected in method body typing (TermNameNotFound) |
| Int assigned to Counter | Invalid, E007 | Rejected in method body typing (ExpectedExpressionTypeMismatch) |
| Extra assignment after the If | Valid | Deferred at the same rejection |
| Nested If | Valid | Deferred at the same rejection |
| Allocation in a branch | Valid | Deferred at the same rejection |
| Assignment in a loop | Valid | Deferred at the same rejection |
| Missing else | Valid | Deferred at the same rejection |
| Forged local owner | Not a Scala source control | Existing mutable-reassignment proof regression rejects WrongOwner |

The official compiler run used -Werror, so invalid controls cannot be
mistaken for warnings. A wrong SymbolId owner is a semantic-input provenance
error rather than a Scala syntax case, so it is covered by the existing
canonical proof mutation regression.

## Dependency ownership and scope

The JVM backend already handles initialized mutable references in If and
While CFGs under [jvm-reference-cfg-contract.md](jvm-reference-cfg-contract.md).
The earlier conditional-selection chain #2265–#2273 audited immutable
If-valued selection and its paired producer. The mutable-rebinding chain
#2298–#2311 audited one straight-line reference assignment. This issue adds
the missing observation where the two Assign nodes are branch-local and the
whole If has Unit result type.

The existing JVM CFG contract remains owned by dotty-jvm; source/typed
identity remains owned by the frontend and SourceTypedIndex; serialization
remains owned by the semantic TASTy pickler; and paired validation/publication
remains owned by the existing paired contract and producer. This audit changes
only tests, fixture evidence, and documentation. It does not modify compiler,
JVM, pickler, contract, publisher, or CI behavior.
