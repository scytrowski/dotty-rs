# Long selectors in JVM and semantic TASTy

Reference: Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main:
`6cd36a0dae7e0f2f2d4002330fb00540330c9826`.

## Scope and increments

1. Freeze this contract and the pinned Scala reference with independent clients.
2. Add symbolic LCmp, encoding, verifier analysis and JDK execution coverage.
3. Lower Long selectors, literal alternatives, guards and binders to JVM code.
4. Validate and emit the corresponding semantic TASTy, including singleton binders.
5. Run Rust-produced original/from-TASTy consumers and repository checks; update gates.

Accept canonical Long formals, inferred/explicit locals, case binders, literal
singletons and existing blocks/ascriptions as selectors. Patterns include Long
literals, nested literal alternatives, wildcard, plain binder, one explicit
binder over a literal/wildcard, `_: Long`, `n: Long`, and `n @ (_: Long)`.
Results retain Int/Boolean/Unit/Long/Float/Double support. Existing Boolean/Int
guards, nested matches, locals, assignments, if/while and supported local returns
compose without expanding their operand-position restrictions.

Evaluate the selector once and each reached guard once. A false guard skips the
whole case, including duplicate alternatives. Long comparisons preserve all 64
bits using LCmp; neither narrowing nor subtraction implements equality. Long
selector temporaries and binder locals occupy two physical slots. Every Long
match needs a final unguarded irrefutable case, even a singleton selector.

Excluded: new Long arithmetic/comparison intrinsics, numeric widening, unsuffixed
Int patterns, Float/Double/reference/Any selectors, boxing/casts, runtime reference
type tests, qualified/aliased/generic patterns, typed/binder alternatives, nested
binders/extractors, partial matches/MatchError, general reachability, switch
optimization, new fields/constructors/calls and discarded Long values requiring
pop2. Returns in unsupported selector/value/guard operands remain excluded.

## Identity and producer contract

Both paths borrow exact canonical session stores and roots. Preserve source
bindings, scope/declaration identity, explicit order, deterministic output,
read-only validation, limits and rejection without partial artifacts. A Long
singleton binder has a canonical Constant(Long) type; authenticated local reads
may yield the existing Long value family without mutating or fabricating trees.

TASTy preserves LONGCONST pattern leaves, ALTERNATIVE, BIND, TYPED and genuine
source edges. Implicit `n: Long` uses the existing source Typed/Ident-to-Bind
mapping; its synthetic Typed/wildcard gain no invented source mappings. Singleton
binder types require LONGCONST in the pattern type position.

## Reference and acceptance

The 51-method reference contains 55 matches, 93 cases, 38 binders, 23 typed
patterns and 15 returns. Scala inserts additional widening wrappers; Rust keeps
its canonical shapes. Independent Java and Scala clients each perform 1122
checks on original and separately rebuilt from-TASTy classes with `-Xverify:all`.
They distinguish Long extrema and values with identical low 32 bits, test signed
floating zeros, guard/selector/body effects, duplicate alternatives, shadowing,
singletons, nested matches and return flow. The runner authenticates provenance
and confirms the original class/TASTy hashes remain unchanged.

Scala-produced reference evidence is separate from acceptance of Rust output.
Extend the existing mandatory `jvm_tasty_long_values` target and independent
`jvm_source_compilation` JVM target rather than create a new strict shard target.

## Implemented verification

The real source pipeline passes the independent 1122-check Java client without
producing TASTy. Paired Rust output passes the four original/from-TASTy Java and
Scala consumer runs, with original artifact hashes unchanged. Fresh sessions
produce identical class and TASTy bytes. Rust preserves 55 MATCH, 93 CASEDEF,
38 BIND, 23 pattern TYPED plus two expression ascriptions, and 15 RETURN nodes.

Focused tests pin LCmp opcode 0x94, physical stack depth four, signed comparison
results, exact descriptors/instructions/frames and two-slot selector/binder
layouts. They reject mismatched full-width constants, partial matches, forged
source edges, malformed type trees and cross-case binding evidence without
mutating canonical stores. Expression budgets and atomic local-slot exhaustion
remain explicit. Singleton binder types and references retain all 64 bits.

The existing mandatory Long target now runs 23 tests. With JDK 21, pinned Scala
3.9 and serial libtest execution it measured 11.88 seconds locally. The strict
inventory remains 38 targets with unchanged assignments; shard 3's estimate
includes the increase. LCmp follows the Java 21 [instruction contract](https://docs.oracle.com/javase/specs/jvms/se21/html/jvms-6.html#jvms-6.5.lcmp).

The full workspace check passes 8670 tests (57 existing ignored) and 44
all-feature doctests. Formatting, frontend dependency boundaries, Clippy and
rustdoc with warnings denied also pass.
