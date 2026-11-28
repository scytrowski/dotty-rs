# Boolean eager and equality operators in JVM and semantic TASTy

Reference: Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main:
`fcaa26024f62e6fac2bf025a772f98b463fe27ad`.

## Scope and increments

1. Pin this contract, the Scala reference and independent consumers.
2. Validate canonical infix Boolean `&`, `|`, `^`, `==` and `!=` applications.
3. Project their signed Boolean selections and emit complete TASTy expressions.
4. Verify original Rust JVM/TASTy and separately rebuilt from-TASTy compositions.
5. Update documentation, mandatory gates and measured timings; run all checks.

Extend the existing primitive expression subset with these five operators on
canonical Boolean operands. Reuse existing literals, formals, visible ordinary
and case locals, ascriptions, supported If/Block/Match values, guards, selectors,
loop conditions, local assignments, primitive result families, expected-Unit
adaptation, statement discard and ordinary local returns outside operands.

Every normally completing operand of these five operators evaluates once, left
to right, including when the left value determines the logical answer. Preserve
effects, exceptions and the existing short-circuit behavior of `&&` and `||`.
Preserve Boolean identity and Z descriptors despite Integer verifier categories.

Excluded: new call/field/constructor signatures, general Boolean-returning calls,
Any/reference equality, boxing, conversions, numeric arithmetic, missing Int
`|`/`^`, explicit operator application syntax such as `a.&(b)`, operand/condition/
guard/initializer returns, non-local returns and unsupported ordinary nested
same-name shadowing. A singleton-typed case binder may supply an RHS value, but
using it as an operator receiver remains unsupported by the source typer.
The independent pre-existing TASTy failure for literal-Unit
If branches (`if flag then () else ()`) remains outside this increment; supported
Unit assignments, adaptations and matches retain their existing contracts.

## Identity and producer contract

Borrow canonical session stores, exact source/typed roots and bindings. Validate
registered operator IDs, ownership/origin, signature, selection name and prefix,
argument count, Boolean operand/result types, spans and source ordering. Preserve
local/case scopes, recursion/work limits and deterministic read-only rejection
without partial complete artifacts. Never identify an intrinsic by spelling alone.

Boolean `==`/`!=` have their own canonical Boolean members. Unlike the existing
Int specialization of Any equality, these project directly as Boolean selections.
The pinned producer shape is APPLY with SELECTin, signed `(scala.Boolean):
scala.Boolean`, the original receiver and a scala.Boolean owner, followed by the
original RHS. Reuse existing wire tags and primitive projection contracts.

The independent JVM already lowers `&`/`|`/`^` as IAND/IOR/IXOR and equality as
IF_ICMPEQ/IF_ICMPNE with Boolean joins. Reuse that lowering and its strict operand
validation; this increment requires no new class-file instructions or frontend
contracts. Paired support must preserve the same behavior in Rust output and
Scala rebuilding from its TASTy.

## Reference and acceptance

The Scala reference has 105 methods: 75 new-operator compositions and 30 existing
short-circuit controls. Independent Java/Scala clients each execute 448 checks
against original and separately rebuilt from-TASTy classes with `-Xverify:all`.
Their expected truth tables are independent constants; ordered effects, discarded
values, exceptions and early returns distinguish eager and short-circuit paths.
Reference generation is Scala-produced evidence, not Rust acceptance.
The clients iterate over all four Boolean argument pairs. `oracle.txt` retains
only the seven formal-operand DEFDEF excerpts from Scala's `-Yprint-tasty` dump;
wire addresses and shared references remain unchanged. Regenerate the excerpts
and provenance by running the fixture's `run.py` without a candidate argument.

Candidate mode must consume actual Rust artifacts, compile independent clients,
execute both original and from-TASTy classes and authenticate unchanged original
hashes. Structural Rust decoding alone does not establish Scala interoperability.

Extend the existing mandatory `jvm_tasty_expressions` target in shard 3, retaining
the 38-target inventory and assignments. Add focused validation/projection tests,
exact descriptors/instructions/frames, identity/scope/limit mutation negatives and
fresh-session determinism. Run complete repository checks, mandatory JDK 21 and
all strict TASTy shards, follow-ups and aggregate before completion.

## Implemented verification

The existing strict expression target now has 40 tests. Ten new regressions cover
all 105 fixture methods individually and together, exact opcodes/descriptors and
comparison stack maps, signed names and owners, distinct Boolean/Int equality
source symbols, operand/value compositions, all existing primitive result widths,
unsupported boundaries, missing source edges, projection limits and deterministic
single/batch output. The pinned consumer accepts actual Rust artifacts and checks
unchanged originals after all four 448-check executions.

Focused source validation also checks ten tree mutations per new operator and
expression depth exhaustion. Projection tests cover all eight Boolean member
identities, exact wire shapes, wrong metadata/receiver/parameter/result/arity,
incomplete or malformed signatures, read-only failure and poisoned-context
rejection. Standard repository, mandatory JDK and all 38 strict TASTy gates are
required in addition to this focused evidence.
