# Scala `Long` and JVM category-two audit

This document records the bounded Scala `Long` support in the JVM backend.
It does not change Typer behavior.
The source compatibility reference is Scala 3.9.0 at repository-pinned
revision `777528f19a58e794c9954a42f433373472ec57f8`.

## Findings

| Layer | Existing behavior |
| --- | --- |
| `dotty-core` / Typer | Canonical `Definitions.long` and `Constant::Long` exist. The source fixture below parses, names, and types, retaining `Long` parameter/result/local types and the exact `9000000000L` literal. |
| `dotty-jvm` projection | Method and constructor metadata project canonical `Definitions.long` as `JvmType::Long`, descriptor `J`, and width two. Instance parameter layouts account for receiver and parameter widths. #1516 lowers canonical Long literals, parameter reads, immutable/mutable local allocation and access, assignment, and method returns in straight-line bodies. |
| Class-file descriptors | The descriptor parser accepts `J`; the linear bytecode analyzer already counts `Long` parameters/values as two slots and tracks the unusable second local slot. |
| Stack-map wire model | `VerificationTypeInfo::Long` uses verification tag 4 and has a two-slot local width. A stack-map frame stores one verification entry for a `Long`, not two entries. |
| CFG analyzer | Supports `Long` parameters, constants, local loads/stores, `lreturn`, branch joins, loop backedges, physical slot accounting, and StackMapTable projection. Long arithmetic, category-two stack manipulation, fields, and calls remain outside this increment. |

`LongValues.scala` exercises the emitted subset through the real source pipeline; its `Runner` methods include zero/one/nontrivial literals, mixed-width parameters, immutable and mutable locals, assignment, and explicit/implicit Long returns. The JDK gate checks decoded instructions and `max_locals`/`max_stack`, inspects the class with `javap`, and runs signed 64-bit values with `java -Xverify:all`. `LongCategoryTwo.scala` remains an audit fixture for fields and branching cases that are deliberately outside this issue.

The current source frontend cannot reach backend validation for Long arithmetic:
`value + 1L` stops in `SourceTyper` with `MemberLookupError::ClassInfoUnavailable`
for `scala.Long`. #1516 records this frontend blocker and leaves `dotty-typer`
unchanged.

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
`bytecode_analysis.rs` checks `J` field operations. CFG behavior is covered by
`cfg_analysis.rs`, including `long_parameter_branch_join_uses_physical_slots_and_one_frame_entry`,
`long_locals_are_pair_aware_at_joins_and_overlapping_stores`, and
`stable_long_local_converges_across_a_counted_loop_backedge`.

## Bounded CFG contract implemented by #1515

The CFG implementation should keep the existing separation between logical
stack values and physical local slots. `FlowValue::Long` is one logical
operand-stack value with width two for stack depth and `max_stack`. Physical
locals use `FlowLocalState::Long` and `FlowLocalState::LongTail` as an
unambiguous pair; the tail is not a readable value. A local `lload` is valid
only when the selected slot is a head immediately followed by its matching
tail. An orphan head or tail is an invalid analyzer state, not a value to
guess through.

For bounded local stores, use JVM overlap invalidation. An `istore` or
reference store at either half of a `Long` first invalidates the entire old
head/tail pair, then writes the category-one value at its destination. An
`lstore` invalidates every existing category-two pair intersecting its two
destination slots, replaces any category-one values in those slots, and then
writes one new head/tail pair. Thus overwriting the old tail with an `Int`
makes the former head unreadable as a `Long`; storing a new `Long` over that
head is valid and creates a new pair. These transitions must also work when
the destination was `Top`, and must respect the method's physical `max_locals`
bound.

Local joins are conservative and pair-aware. A `Long` local survives a join
only when every reachable predecessor has a `Long` head at the same index and
the corresponding `LongTail` at the next index. If any predecessor has
`Top`, a category-one value, or a differently aligned category-two value in
either half, the merged pair is unusable (`Top` at both physical slots).
Unrelated local slots retain their independently joined state. A read after a
join is allowed only from a complete surviving head/tail pair; a value
initialized on only one path is not definitely assigned. A loop backedge
participates in the same monotone join until the fixed point: if it contributes
incompatible or `Top` evidence, the pair widens to unusable and later
iterations cannot restore the earlier `Long` without a store on that path.

Operand-stack joins use logical values and require equal slot depth and
compatible value shape. `Long` joins with `Long`; joining `Long` with a
category-one value or with a different stack shape is rejected. This audit
does not propose general numeric widening, `dup2`/`pop2`, or uninitialized
object handling changes.

Conversion to StackMapTable locals consumes an actual `Long` head and its
matching `LongTail` as one `VerificationTypeInfo::Long` entry. It must retain
each independent `Top` slot as a `Top` verification entry, and reject an
orphan tail. For example, physical locals
`[LongHead, LongTail, Top, Integer]` become logical frame entries
`[Long, Top, Integer]`: skipping the tail must not erase the real hole at
physical slot 2.

| Case | Audit baseline in this PR | Contract coverage owner |
| --- | --- | --- |
| Descriptor `J`, two-slot linear parameter/local accounting, valid head load and tail-load rejection | Positive/negative tests in `bytecode_analysis.rs`; current behavior passes | Existing class-file linear analyzer |
| One StackMapTable Long entry with two physical local slots | Existing `counts_verification_entries_separately_from_slots` test | Existing class-file wire model |
| CFG `Long` parameters, constants, loads/stores, and `lreturn` | Focused positive and negative `cfg_analysis.rs` tests | Implemented by #1515 |
| Pair-aware overlap invalidation, local joins, and unreadable `Top` paths | `long_locals_are_pair_aware_at_joins_and_overlapping_stores` plus linear analyzer tail checks | Implemented by #1515 |
| CFG loop-backedge convergence for stable and invalidated `Long` locals | Stable counted loop coverage plus pair-aware join behavior | Implemented by #1515 |
| Physical-to-logical frame conversion and correct branch PCs | CFG frame projection test; generated-class JDK test when JDK 21+ is available | Implemented by #1515 |

The CFG regression matrix also checks category-one opcode rejection for
category-two values, both orders of an incompatible `Long`/`Integer` stack
join, wrong return descriptors, one-arm initialization, tail overwrite, and
stable Long locals through a loop backedge. `Ldc2` uses the existing validated
constant-pool path; arithmetic, `lcmp`, `pop2`/`dup2`, conversions, and
Long-aware field/method transfer remain separate follow-up work.

## JVMS 21 constraints

The class-file descriptor for `long` is `J` ([JVMS 21 §4.3.3](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.3)). A `long` value occupies two consecutive local-variable slots and two operand-stack units; the second local slot is not an independently loadable value. A store to that second slot invalidates the overlapping category-two value ([JVMS 21 §2.6.1](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html#jvms-2.6.1), [§2.6.2](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html#jvms-2.6.2)).

Stack-map frames encode a `Long` as one `verification_type_info` entry with
tag 4, while local-variable slot accounting still counts two slots
([JVMS 21 §4.7.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4)). Method parameter limits also count JVM slots, including the receiver for
instance methods. These are format and verifier constraints, not support
already implied by the existence of a `Long` constant or a wire-level
verification tag.

## Scope for follow-up issues

The JVM Backend issues split implementation into descriptors and slot
allocation (#1514), CFG category-two state (#1515), literal/local/return
lowering (#1516), fields and constructors (#1517), calls and `If`/`While`
control flow (#1518), and end-to-end execution (#1519). Issues #1514–#1516
provide the descriptor, CFG, and straight-line value foundation. Long fields,
method calls, arithmetic, category-two stack manipulation, and Long-aware
control-flow expressions remain follow-up work; Typer is unchanged.
