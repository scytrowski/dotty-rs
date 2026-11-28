# Primitive value discard in JVM and semantic TASTy

Reference: Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main:
`046f0aac5f27c1115b84d8ce8bfb685cdcde4775`.

## Scope and increments

1. Pin this contract, Scala fixtures, independent clients and failure reproduction.
2. Add POP2 encoding, both verifier forms and low-level JDK tests.
3. Lower primitive statement values in blocks/loops and fix Double while-body discard.
4. Validate semantic TASTy statements and authentic expected-Unit adaptations.
5. Accept Rust originals/from-TASTy output, run complete checks and update gates/docs.

Accept already supported Int/Boolean/Long/Float/Double value expressions used as
non-final block statements, primitive-valued While bodies and expected-Unit
adaptations in ordinary explicitly typed methods. Existing blocks, ascriptions,
If/Match, scopes, guards, selectors and supported local returns compose within
their current operand restrictions. Existing authenticated Int calls may be reused
without extending call signatures or receiver support.

Evaluate statements once in source order, preserving effects, conditional paths,
exceptions and supported exits. A normal discarded Int/Boolean/Float consumes
one physical slot with POP; Long/Double consume two with POP2. Unit and diverging
paths supply no value to pop. The final expression retains its exact result kind.
This also fixes the current Double-valued While-body path that emits POP and is
then correctly rejected by the class-file analyzer as PopCategoryTwo.

Excluded: source Unit locals with primitive initializers (currently rejected by
the typer), reference/null/String/Any discard, boxing, new arithmetic/operators,
widening/conversions, new Boolean eager/equality/call semantics, new wide calls,
fields/constructors/ABI shapes, non-local returns and returns inside unsupported
value/guard/initializer operands. Purity/discard warnings, general reachability
and identical optimization to Scala are outside this contract.

## Identity and output contract

Borrow canonical stores, typed roots and exact source bindings. Expected-Unit
adaptation already exists as Block(stats=[original], expr=Unit). Validate its
actual shape, types and spans without changing the original value's type or
manufacturing source mappings. Expected-expression adaptation maps the source
root to the wrapper; incomplete If branch adaptation retains its source mapping
to the original child. Keep both cases explicit and authenticated. Preserve local
and pattern declaration identity, visibility, work limits and read-only rejection
without partial artifacts. While bodies may retain their canonical primitive
type; verify the emitted producer shape with the pinned Scala consumer.

TASTy reuses BLOCK, UNITCONST and existing primitive expression tags. Structural
parseability alone does not establish interoperability. Scala may remove pure
loads or put Unit wrappers at different positions; require semantic compatibility
and effect preservation rather than identical trees or class bytes.

POP2 implements both JVMS forms: one category-two value or two category-one
values. Validate the complete shape before removing kinds and parallel reference,
null, join and allocation metadata. Reject underflow or a category-one top above
a category-two value with typed errors. Constructor prefix restrictions remain.

## Reference and acceptance

The 104-method Scala reference covers 100 primitive composition cases and four
exception controls. Independent Java and Scala clients each perform 1928 checks
against original and separately rebuilt from-TASTy classes with -Xverify:all.
They include signed zeros, non-finite inputs, preserved final results, ordered
effects, conditional/loop execution, ArithmeticException and local return flow.
The runner authenticates provenance and unchanged original artifact hashes.

Extend the existing mandatory jvm_tasty_expressions target and independent
jvm_source_compilation target. Keep the 38-target strict inventory and shard
assignments; update measured target/shard timings after implementation.
