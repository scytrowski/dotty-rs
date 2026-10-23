# Scala `Long` and JVM category-two audit

This audit records the existing support boundary for Scala `Long` in the JVM
backend. It does not enable `Long` code generation or change Typer behavior.
The source compatibility reference is Scala 3.9.0 at repository-pinned
revision `777528f19a58e794c9954a42f433373472ec57f8`.

## Findings

| Layer | Existing behavior |
| --- | --- |
| `dotty-core` / Typer | Canonical `Definitions.long` and `Constant::Long` exist. The source fixture below parses, names, and types, retaining `Long` parameter/result/local types and the exact `9000000000L` literal. |
| `dotty-jvm` projection | Method and constructor metadata project canonical `Definitions.long` as `JvmType::Long`, descriptor `J`, and width two. Instance parameter layouts account for the receiver and each parameter's actual width. Long expression, field, local allocation, and code emission remain unsupported. |
| Class-file descriptors | The descriptor parser accepts `J`; the linear bytecode analyzer already counts `Long` parameters/values as two slots and tracks the unusable second local slot. |
| Stack-map wire model | `VerificationTypeInfo::Long` uses verification tag 4 and has a two-slot local width. A stack-map frame stores one verification entry for a `Long`, not two entries. |
| CFG analyzer | Its flow state has no `Long` value/local variant. It rejects a `Long` method parameter as `UnsupportedFlowParameter` before CFG transfer analysis. |

The source characterization test is `scala_long_source_reaches_typed_plans_before_backend_rejects_it` in `tests/jvm_source_compilation.rs`. Its observer runs after successful parsing, naming, typing, and JVM plan construction. All checked source signatures and `saved` locals use canonical `Definitions.long`; the `constant` body retains `Constant::Long(9_000_000_000)` and a source position. The focused `LongDescriptors.scala` regression verifies same-session metadata for `(J)V`, `(J)J`, and mixed `(ZIJI)J` signatures, including constructor and method local slots. Emission still fails with a typed unsupported-expression error and publishes no class file. Metadata projection is implemented while value lowering remains a later increment.

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

## Bounded CFG contract for follow-up #1515

The CFG implementation should keep the existing separation between logical
stack values and physical local slots. `FlowValue::Long` is one logical
operand-stack value with width two for stack depth and `max_stack`. Physical
locals need distinct `LongHead` and `LongTail` states (or an equivalent
unambiguous pair); the tail is not a readable value. A local `lload` is valid
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
only when every reachable predecessor has a `LongHead` at the same index and
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

Conversion to StackMapTable locals must consume an actual `LongHead` and its
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
| CFG `Long` parameter and `lconst_0` transfer | Existing tests reject these as unsupported | Current CFG analyzer boundary |
| CFG head/tail initialization, overlap invalidation, local joins and unreadable `Top` paths | Not implemented or claimed by this audit | Follow-up #1515 tests and CFG implementation |
| CFG loop-backedge convergence for a stable `Long` and for incompatible/`Top` evidence | Not implemented or claimed by this audit | Follow-up #1515 tests and CFG implementation |
| Physical-to-logical frame conversion preserving genuine `Top` gaps | Not implemented or claimed by this audit | Follow-up #1515 tests and frame generation |

The future CFG positive matrix should cover: a `Long` parameter loaded from
its head; a local initialized by `lstore` then loaded; category-one and
category-two overwrites of an existing pair; identical `Long` states joined
across both arms and a loop backedge; and frame conversion with a `Top` gap.
Negative coverage should include: loading the tail or an orphan head, a
category-one overwrite followed by `lload`, a `Long` initialized on only one
predecessor then read after the join, incompatible stack joins, malformed
head/tail states, out-of-bounds pairs, and a loop backedge that invalidates a
previously initialized pair. These are follow-up acceptance cases, not claims
that the current CFG analyzer supports them.

## JVMS 21 constraints

The class-file descriptor for `long` is `J` ([JVMS 21 §4.3.3](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.3.3)). A `long` value occupies two consecutive local-variable slots and two operand-stack units; the second local slot is not an independently loadable value. A store to that second slot invalidates the overlapping category-two value ([JVMS 21 §2.6.1](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html#jvms-2.6.1), [§2.6.2](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-2.html#jvms-2.6.2)).

Stack-map frames encode a `Long` as one `verification_type_info` entry with
tag 4, while local-variable slot accounting still counts two slots
([JVMS 21 §4.7.4](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-4.html#jvms-4.7.4)). Method parameter limits also count JVM slots, including the receiver for
instance methods. These are format and verifier constraints, not support
already implied by the existence of a `Long` constant or a wire-level
verification tag.

## Scope for follow-up issues

The subsequent JVM Backend issues split implementation into CFG category-two
state (#1515), literal/local/return lowering (#1516), fields and constructors
(#1517), calls and `If`/`While` control flow (#1518), and end-to-end execution
(#1519). The descriptor and parameter-width metadata increment (#1514) is
implemented. This audit does not implement the remaining increments,
`dup2`/`pop2`, general verifier joins, or any source Typer changes.
