# Float/Double selectors in JVM and semantic TASTy

Reference: Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main:
`edd5387bf11cb8026ca01f6d2592f5db892f59b3`.

## Scope and increments

1. Freeze this contract and the pinned Scala reference with independent clients.
2. Add FCmpL/DCmpL, encoding, verifier analysis and JDK execution coverage.
3. Lower Float/Double selectors, alternatives, guards and binders to JVM code.
4. Validate and emit semantic TASTy, including exact singleton binder types.
5. Run Rust-produced original/from-TASTy consumers and repository checks; update gates.

Accept canonical Float/Double formals, inferred/explicit locals, case binders,
accepted literal singletons and existing blocks/ascriptions as selectors.
Patterns include same-family finite literals, ordered/nested literal alternatives,
wildcard, plain binder, one binder over a literal/wildcard, `_: Float`, `n: Float`,
`n @ (_: Float)` and Double equivalents. Existing unsuffixed Double patterns
remain supported. Results retain Int/Boolean/Unit/Long/Float/Double support.
Existing Boolean/Int guards, nested matches, locals, assignments, if/while and
supported local returns compose within their existing operand restrictions.

Evaluate the selector once and each reached guard once. A false guard skips the
whole case, including numerically equal signed-zero alternatives. Runtime equality
uses FCmpL/DCmpL: positive and negative zero compare equal; NaN never matches a
literal. Irrefutable cases accept NaN. Binders copy the original selector,
preserving its signed zero; NaN acceptance checks classification, not payload
fidelity. Float locals occupy one physical slot; Double locals occupy two.
Every floating match needs a final unguarded irrefutable case, even a singleton.

Excluded: floating arithmetic/comparison intrinsics, widening or mixed numeric
patterns, qualified/aliased/generic patterns, stable NaN/infinity patterns,
boxing/reference tests, typed/binder alternatives, nested binders/extractors,
partial matches/MatchError, general exhaustivity/reachability, switches, new
fields/constructors/calls, discarded category-two values and returns in unsupported
selector/value/guard operands. Opposite-sign zero patterns on literal singleton
selectors that the frontend already rejects remain excluded; this sprint does
not change the typer's raw Constant identity rules.

## Identity and producer contract

Both output paths borrow canonical session stores and roots. Preserve source
bindings, scope/declaration identity, explicit order, deterministic output,
read-only validation, limits and rejection without partial artifacts.
Literal/source/type/TASTy validation uses exact raw bits, independently of runtime
numeric equality. Singleton binder types retain FloatBits/DoubleBits; authenticated
local reads may use their corresponding primitive value family.

TASTy preserves FLOATCONST/DOUBLECONST pattern and singleton-type leaves,
ALTERNATIVE, BIND, TYPED and genuine source edges. The existing implicit typed
binder mapping does not invent mappings for synthetic Typed/wildcard children.

## Reference and acceptance

The 103-method reference contains 111 matches, 196 cases, 76 binders, 42 typed
patterns and 30 returns. Scala inserts extra widening wrappers; Rust retains
its canonical shapes (42 pattern TYPED and two expression ascriptions).
Independent Java and Scala clients each perform 3502 checks on original and
separately rebuilt from-TASTy classes with `-Xverify:all`. Inputs cover infinities,
NaNs, signed zeros, subnormals, finite extrema and adjacent values. Checks cover
case order, selector/guard/body effects, false guards on equal-zero alternatives,
shadowing, singleton binders, six result families and supported return flow.
The runner authenticates provenance and unchanged original class/TASTy hashes.

Scala-produced reference evidence is separate from Rust-output acceptance.
Extend the existing mandatory `jvm_tasty_floating_values` target and independent
`jvm_source_compilation` target without adding a strict shard target.

## Implemented verification

The independent real-source JVM pipeline passes the 3502-check Java client
without producing TASTy. Paired Rust output passes all four original/from-TASTy
Java/Scala client runs with unchanged original artifact hashes. Fresh sessions
produce identical class and TASTy bytes. Rust preserves 111 MATCH, 196 CASEDEF,
76 BIND, 42 pattern TYPED plus two expression ascriptions, and 30 RETURN nodes.

Focused tests pin FCmpL/DCmpL opcodes 0x95/0x97, their two/four physical stack
slots, same-width wrong-kind rejection, exact descriptors/instructions/frames,
and one/two-slot selector and binder layouts. Raw signed-zero corruption,
malformed wrappers, missing source edges, mismatched singleton joins and
missing/duplicate/cross-case binding evidence reject read-only. Limits and
atomic temporary-slot exhaustion remain explicit. Singleton binders preserve
FLOATCONST/DOUBLECONST types and direct declaration references.

The existing mandatory floating target now runs 24 tests. Its pinned Scala/JDK
serial execution measured 14.21 seconds locally. The strict inventory remains
38 targets with unchanged shard assignments; shard 4's estimate includes the
increase. The new contract is also a strict workflow path trigger.

The full workspace check passes 8689 tests (57 existing ignored) and 44
all-feature doctests. Formatting, frontend boundaries, Clippy and rustdoc with
warnings denied, the standalone compiler client and workflow checks also pass.
