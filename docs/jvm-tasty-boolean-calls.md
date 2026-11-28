# Same-class Int/Boolean method calls in JVM and TASTy

Reference: Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0 and JVM class-file major 65. Base main: `9f971787c2`.

## Scope and increments

Support ordinary public source methods in the same class, with one plain value
parameter list (including `()`) and canonical Int/Boolean parameters and results.
Calls use implicit current `this` or explicit unqualified `this`. Preserve the
existing Int call paths, contextual Int calls and bounded reference factories.

1. Pin this contract and a compact Scala-generated reference.
2. Validate canonical targets, exact signatures and source/typed argument edges.
3. Emit signed names and SELECTin/APPLY with the original argument order.
4. Verify compositions, identity/limit failures and deterministic paired output.
5. Consume actual Rust originals and separately rebuilt from-TASTy artifacts;
   update the existing strict expression target, documentation and CI evidence.

Reuse existing Boolean operators, locals, conditions, guards, selectors, loops,
Unit adaptation/discard and ordinary local returns. Normally completing call
arguments evaluate once, left to right. Callee effects and exceptions must remain
observable; `&&`/`||` keep their short-circuit behavior around calls.

Retain exact canonical method, class and parameter symbols, source/typed roots,
function/receiver/argument bindings, declaration order and lexical scopes.
Validation is read-only; malformed or unsupported plans cannot escape as partial
complete artifacts. Keep existing work and recursion limits.

New cross-class, inherited/super, private, parameterless-getter, inferred-result,
curried/contextual, by-name, default, varargs and polymorphic call shapes are
outside this increment. New calls using Unit/Long/Float/Double/reference signatures,
boxing and conversions are also excluded. Existing operand-return and literal-Unit
If restrictions remain explicit.

## Reference and acceptance

The 30-method `BooleanCalls.scala` fixture and looped Java/Scala clients execute
78 checks each. The adjacent `run.py` authenticates pinned toolchain artifacts,
preserves original hashes, reads TASTy and tests original and from-TASTy classes.
Run it without an argument to regenerate the reference excerpts and provenance;
pass a directory of actual Rust artifacts for producer acceptance. The compact
oracle retains five call trees and their shared Scala package reference.

Scala emits signed method names with exact mixed parameter order and result type,
SELECTin owned by the source class, QUALTHIS for an implicit receiver and THIS for
an explicit receiver, all under APPLY. Existing JVM lowering already supports
`invokevirtual`, `Z` descriptors and Boolean verifier categories; no new bytecode
instruction or frontend contract is needed.

Reference generation is Scala-produced evidence. Acceptance additionally requires
focused validation/projection tests, exact descriptors and call instructions,
identity/arity/type/binding mutations, bounded rejection, fresh-session determinism,
mandatory JDK 21 execution and pinned Scala consumers of actual Rust output.
Retain the existing strict `jvm_tasty_expressions` target and all repository gates.
