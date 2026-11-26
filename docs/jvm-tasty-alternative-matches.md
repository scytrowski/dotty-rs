# Literal Int alternative patterns in JVM and TASTy

Target Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main: `2c2085eb49391406bc140f2696a7a46468e5985d`.

## Scope and increments

1. Freeze the pinned Scala oracle, independent clients and this contract.
2. Validate and lower bounded literal alternatives in JVM patterns.
3. Validate source/typed pattern correspondence and emit semantic TASTy alternatives.
4. Cover composition, exact instructions/frames, determinism and malformed plans.
5. Require independent JDK/Scala consumers and update the strict CI gates.

The selector is Int and evaluated once. Alternative leaves are Int literals;
parentheses, nested alternatives, duplicates and signed Int boundaries are supported.
Each case retains one guard and body. A matching alternative reaches that guard once;
a false guard continues to the next case, never another alternative of the same case.
Cases keep source order and exactly one final unguarded irrefutable case is required.
Existing literal, wildcard and case-binder patterns remain available in other cases.
Results are Int, Boolean, Unit or Long, including already-supported local returns.

Wildcards inside alternatives, bindings over/inside alternatives, extractors,
runtime type tests, non-Int selectors, non-exhaustive matches, switch optimization,
and additional numeric operations are outside this increment. Frontend/core ASTs
and structural ALTERNATIVE encoding already exist; no new parallel representation
or production dependency is needed. This does not add semantic method-body unpickling.

## Evidence and acceptance

`tests/fixtures/jvm_tasty_alternative_matches/run.py` uses the pinned offline
Scala toolchain. Its no-argument mode creates Scala reference evidence; candidate
mode must consume actual Rust artifacts separately. Semantic/runtime compatibility
is required, not byte equality with Scala output. Use the semantic decompiler rather
than the pinned raw numeric printer, which mishandles signed constants.

The fixture covers guard hits/misses, duplicate alternatives, false-guard continuation,
selector/guard effects, nested patterns/matches, existing binders, local values,
loop returns, Boolean/Unit results and Long.MIN/MAX values. Independent Java and Scala
clients consume the original pair; Java also executes classes rebuilt by `-from-tasty`.
Limits, source/typed identity and read-only failure must remain enforced.
