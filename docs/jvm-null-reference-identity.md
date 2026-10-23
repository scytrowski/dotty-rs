# JVM null and reference identity audit

This is an audit checkpoint for Scala 3.9.0 default-null semantics and the
Java SE 21 verifier contract. It records current source and classfile boundaries
and successor ownership. The source typing increment in #1469 is implemented
below; identity-call and bytecode support remain separate work.

## Reference evidence

The Scala source reference is pinned to
[`777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8).
At that revision, the [Scala expression specification](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/docs/_spec/06-expressions.md)
states that `null` has type `scala.Null` and conforms to every reference type.
For the null value, it specifies `eq` and `==` as true exactly when the
argument is also null, and `ne` and `!=` as true when the argument is not null;
ordinary member access on that value throws `NullPointerException`. The same
specification separately defines null-safe intrinsics: `isInstanceOf` returns
`false`, `asInstanceOf` returns the target type's default value, and `##`
returns zero. These intrinsic behaviors are recorded here as compatibility
facts, not implemented by this audit.
[`Definitions.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/core/Definitions.scala)
models `AnyRef` as an alias of `Object`, declares `Object_eq` and `Object_ne`
with signature `(AnyRef): Boolean`, and declares `Null` as a class whose
default-mode parent is `Object`. Under explicit-nulls mode its parent changes
to `Any` and `Matchable`; that is a separate mode, not the default semantics
audited here.

The pinned compiler keeps a separate `scala.runtime.Null$` symbol as the
runtime manifestation of source `scala.Null` in method signatures only;
[`BTypeLoader.scala`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/backend/jvm/BTypeLoader.scala)
maps `NullClass` to that signature class. This mapping does not mean the `null`
literal allocates or refers to a `scala.runtime.Null$` object: the language
specification describes the literal as the special null reference. Keep the
source `Null` symbol, signature mapping, and verifier `Null` state as separate
identities.

`eq`/`ne` are referential identity operations. They must not be recovered from
operator spelling or mapped to universal `Any.==`/`Any.!=`; universal equality
has different Scala semantics even though the null receiver's specified result
coincides with identity for these cases. `==` and `!=` are not substitutes for
the identity contract.

The verifier reference is the [Java SE 21 JVMS](https://docs.oracle.com/javase/specs/jvms/se21/html/):
[§4.10.1.2](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.2)
defines `null` as a verification type below Java reference types and permits it
as an assignment to class and array reference types;
[§4.7.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4)
encodes the distinct `Null_variable_info` tag 5;
[§4.10.1.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.10.1.4)
defines frame transitions and assignment checks; and
[§6.5](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html)
defines `if_acmpeq`, `if_acmpne`, `ifnull`, and `ifnonnull`.

The pinned Scala source files and spec text were inspected directly. A local
Scala 3.9.0 compiler run was unavailable: this environment has no Scala
launcher, pinned compiler JAR, `SCALA39_ROOT`, or `SCALA39_CLASSPATH`. Thus the
source-backed expectations below are grounded in the pinned compiler sources
and language specification, while an executable Scala oracle comparison
remains explicitly unverified.

## Source characterization

[`tests/fixtures/jvm/NullReferenceIdentity.scala`](../tests/fixtures/jvm/NullReferenceIdentity.scala)
preserves the candidate source shape: a `Box`, identity methods, a nullable
`if` result, and a null-sensitive read. The focused Typer regression runs it
through the real lexer, parser, namer, and source Typer in one semantic session.
Lexing, parsing, and naming succeed. The present first failures are:

| Method | First failing stage | Current result |
| --- | --- | --- |
| `same(a, b)` | Typer member lookup | `MemberNotFound` for `eq` on `Box` |
| `different(a, b)` | Typer member lookup | `MemberNotFound` for `ne` on `Box` |
| `choose(flag, value)` | Typer | Accepted with a typed `Box` result; the `null` branch retains canonical source `Null` |
| `readOrDefault(flag, value)` | Typer member lookup | `MemberNotFound` for `eq` on `Box` |

Each failed expression leaves no typed root in the source typed index. The
current error ordering matters: `readOrDefault` reaches the identity selection
first; its null-sensitive branch and dereference are not evidence of existing
null flow analysis. No code generation is attempted after these Typer failures.

In default Scala 3.9 semantics, `null` has source type `Null` and conforms to
reference types. This increment supports explicit `Null` results, inferred bare
`null` results, local values annotated with an ordinary source class, and `If`
joins between `null` and a non-generic top-level class from the current source
unit (in either branch order). It also supports `Object`; it does not add
general union erasure, generic or external-class conformance, parameter or
field adaptation, or flow-sensitive narrowing. The verifier's permission to
store or return a null reference does not imply JVM lowering support.
Dereference has ordinary runtime null behavior; the compiler must not silently
add a guard. `-Yexplicit-nulls` remains excluded.

## Current implementation boundary

`dotty-core::Definitions` owns a canonical `Null` symbol and type distinct from
`Nothing`, `Any`, and `Object`. Source `Constant::Null` literals use that type.
The typer's bounded relation accepts `Null` for `Object` and current-unit,
non-generic top-level source classes with canonical prefixes; it rejects
primitive targets and unsupported class shapes. `AnyRef` aliases and inherited
`Object.eq`/`Object.ne` source method identities are not implemented, so member
resolution still cannot select canonical identity method IDs.

At the classfile layer:

- `Instruction::AConstNull` exists in the typed instruction model.
- `VerificationTypeInfo::Null` serializes as the JVMS tag 5; the existing
  `encodes_every_verification_type_tag` regression checks that byte.
- The exact CFG state model has `FlowValue::Integer`/`Object(ClassIdentity)` on
  the stack and `Top`/`Integer`/`Receiver`/`Object(ClassIdentity)` in locals.
  It has no `Null` state. The context-free `CfgCodeAnalyzer::analyze`
  entrypoint rejects `aconst_null; areturn` with
  `UnsupportedFlowInstructionKind::Reference` because that entrypoint does not
  admit reference flow.
- The class-aware straight-line frame uses a generic reference category with
  no class identity for `aconst_null`; it cannot preserve `Null` distinctly
  from an unclassified reference value. `CfgCodeAnalyzer::analyze_with_current_class`
  admits `aconst_null` as that unclassified reference, but
  `aconst_null; areturn` still fails at instruction 1 with
  `MissingExactReferenceReturn`. The in-block sequence
  `aconst_null; astore; aload; pop; return` is accepted there. These outcomes
  show a difference between instruction admission and exact null-flow identity;
  neither provides first-class `Null` CFG support.
- The current typed branch model handles integer conditions and integer
  comparisons. It has no reference equality or null-check branch variants.
- `Object_eq`/`Object_ne` are Scala semantic method identities, not methods to
  invoke on Java `Object`. Their bounded JVM implementation must use the
  reference comparison branch contract below, not `invokevirtual` or
  universal `equals` semantics.

The wire-format support for a `Null` stack-map item is therefore not evidence
that CFG analysis, frame generation, source typing, or lowering supports null.

## Required verifier contract for successor work

Keep the verifier value `Null` distinct from `Object(C)`, `Top`,
`Uninitialized(offset)`, and `UninitializedThis`:

| Incoming state | Required bounded result |
| --- | --- |
| `Null` + `Null` | `Null` |
| `Null` + `Object(C)` | `Object(C)` is a valid join/frame type because JVMS permits `Null` to be assigned to `C`; no invented class identity is attached to the null value itself |
| `Null` + `Object(A)` + `Object(B)` | Preserve the real object classes as join evidence; a class result requires the same checked common-ancestor proof as the non-null inputs |
| `Null` + `Top` local | Keep `Top`; a null store on one path cannot make an unassigned local readable |
| `Null` + `Uninitialized(offset)` or `UninitializedThis` | Reject as incompatible; null is not an initialized allocation and cannot complete construction |
| `Null` assigned to primitive descriptor or compared with a primitive | Reject |

`aload`, `astore`, reference arguments and `areturn` may eventually carry a
null value only under a validated reference context and JVMS assignment rules.
The `Null` stack-map entry has no constant-pool index; a widened `Object(C)`
entry must use the validated `CONSTANT_Class` entry for `C`.

`if_acmpeq`/`if_acmpne` consume two category-one reference values from the
operand stack (the right/top value first, then the left) and branch on identity.
`ifnull`/`ifnonnull` consume one reference value. Their branch offsets are
relative to the opcode PC and target instruction boundaries; the assembler
must bind symbolic labels and frame targets rather than accept guessed byte
offsets. A null check does not itself refine the Scala source type in this
increment.

Reject primitive/reference comparison, unsupported or foreign member IDs,
operator-spelling recovery, `==`/`!=` rewrites, missing hierarchy evidence,
`Null` forged as `Object(C)`, `Null`/`Top` or uninitialized promotion,
unsupported generics/external classes, arrays, traits/interfaces, exception
edges, and branched constructors. Do not add dereference guards. Explicit-null
types and flow-sensitive narrowing remain separate work.

## Successor ownership and merge order

All successors target milestone **JVM Backend**. This audit is the prerequisite
for the sequence below; the entries are planned ownership, not implementation
claims.

| Issue | Owning layer and scope | Dependencies |
| --- | --- | --- |
| #1469 | `dotty-core`/`dotty-typer`: canonical source `Null` identity and limited default Scala 3.9 typing | Implemented in this increment |
| #1470 | `dotty-classfile`: Scala-independent `Null` verifier flow, joins, and stack-map generation | This audit; may run alongside #1469 |
| #1471 | `dotty-classfile`: reference compare/null branch instructions, symbolic assembly, and CFG transfer | This audit; may run alongside #1469/#1470; final null-branch tests require #1470 |
| #1472 | `dotty-core`/`dotty-typer`: canonical `AnyRef.eq/ne` member IDs and normal Typer selection | This audit; may run alongside #1471 and non-null parts of #1469 |
| #1473 | `dotty-jvm`: source-typed null lowering in explicit reference contexts | #1469 and #1470 |
| #1474 | `dotty-jvm`: identity-only `eq/ne` lowering and Boolean materialization | #1471, #1472, and #1473 (therefore #1470) |
| #1475 | Real source/JDK 21+ end-to-end acceptance, verifier frames, deterministic classes, and negative first-stage diagnostics | #1469–#1474 |

`dotty-classfile` remains Scala-independent; source identities stay in
`dotty-core`/`dotty-typer`; `dotty-jvm` only lowers validated canonical typed
trees. No successor may recognize `eq`/`ne` by text or broaden the intrinsic to
universal equality.
