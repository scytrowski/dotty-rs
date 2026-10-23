# Scala `Long` and JVM category-two audit

This audit records the existing support boundary for Scala `Long` in the JVM
backend. It does not enable `Long` code generation or change Typer behavior.
The source compatibility reference is Scala 3.9.0 at repository-pinned
revision `777528f19a58e794c9954a42f433373472ec57f8`.

## Findings

| Layer | Existing behavior |
| --- | --- |
| `dotty-core` / Typer | Canonical `Definitions.long` and `Constant::Long` exist. The source fixture below parses, names, and types, retaining `Long` parameter/result/local types and the exact `9000000000L` literal. |
| `dotty-jvm` projection | `JvmType` currently represents `Int`, `Boolean`, and planned class references. Descriptor projection and local allocation therefore have no `Long` representation or width. The allocator reserves one slot per supported parameter/local. |
| Class-file descriptors | The descriptor parser accepts `J`; the linear bytecode analyzer already counts `Long` parameters/values as two slots and tracks the unusable second local slot. |
| Stack-map wire model | `VerificationTypeInfo::Long` uses verification tag 4 and has a two-slot local width. A stack-map frame stores one verification entry for a `Long`, not two entries. |
| CFG analyzer | Its flow state has no `Long` value/local variant. It rejects a `Long` method parameter as `UnsupportedFlowParameter` before CFG transfer analysis. |

The source characterization test is `scala_long_source_reaches_typed_plans_before_backend_rejects_it` in `tests/jvm_source_compilation.rs`. Its observer runs after successful parsing, naming, typing, and JVM plan construction. All checked source signatures and `saved` locals use the canonical `Definitions.long`; the `constant` body retains `Constant::Long(9_000_000_000)` and a source position. The end-to-end fixture then fails during backend emission with `UnsupportedJvmReferenceType { kind: ExternalClass }`. That existing external-class boundary is encountered first, so this full fixture does not isolate a later `Long` descriptor or opcode failure. No production behavior was changed to work around it.

## Scala 3.9.0 oracle fixture

`tests/fixtures/jvm/LongCategoryTwo.scala` is compiled with the pinned Scala
3.9.0 compiler. `javap -s` reports these descriptors:

| Source member | JVM descriptor |
| --- | --- |
| `LongBox.<init>(initial: Long)` | `(J)V` |
| `LongBox.get()` | `()J` |
| `LongBox.set(value: Long)` | `(J)V` |
| `Runner.constant()` | `()J` |
| `Runner.roundTrip(value: Long)` | `(J)J` |
| `Runner.choose(flag: Boolean, left: Long, right: Long)` | `(ZJJ)J` |
| `Runner.repeat(n: Int, first: Long, second: Long)` | `(IJJ)J` |

The emitted constructor uses `lload_1` for its `Long` parameter, after
`this` occupies local slot 0. In an instance method with descriptor `(J)J`,
the first parameter starts at local slot 1 and occupies slots 1 and 2. In
`(JI)I`, the following `Int` starts at slot 3. The new linear analyzer test
characterizes these offsets and bounds: `(J)J` on an instance method has
`max_locals = 3`, and `(JI)I` has `max_locals = 4`. Attempting to load the
second slot of the first `Long` is rejected as `LocalSlotOverlap`.

`dotty-classfile` already has related coverage: `stack_map.rs`'s
`counts_verification_entries_separately_from_slots` checks that a `Long`
occupies one verification entry but two local slots; field-width coverage in
`bytecode_analysis.rs` checks `J` field operations. The CFG analyzer's
existing `non_integer_parameters_are_rejected_without_changing_linear_analysis`
and `reference_and_category_two_flows_are_rejected_explicitly` tests pin the
unsupported CFG boundaries.

## JVMS 21 constraints

The class-file descriptor for `long` is `J` ([JVMS 21 §4.3.3](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.3)). A `long` value occupies two consecutive local-variable slots and two operand-stack units; the second local slot is not an independently loadable value. A store to that second slot invalidates the overlapping category-two value ([JVMS 21 §2.6.1](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html#jvms-2.6.1), [§2.6.2](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html#jvms-2.6.2)).

Stack-map frames encode a `Long` as one `verification_type_info` entry with
tag 4, while local-variable slot accounting still counts two slots
([JVMS 21 §4.7.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4)). Method parameter limits also count JVM slots, including the receiver for
instance methods. These are format and verifier constraints, not support
already implied by the existence of a `Long` constant or a wire-level
verification tag.

## Scope for follow-up issues

The subsequent JVM Backend issues split implementation into descriptors and
slot allocation (#1514), CFG category-two state (#1515), literal/local/return
lowering (#1516), fields and constructors (#1517), calls and `If`/`While`
control flow (#1518), and end-to-end execution (#1519). This audit does not
implement those increments, `dup2`/`pop2`, general verifier joins, or any
source Typer changes.
