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
clients consume the original pair and classes rebuilt by `-from-tasty`.
Limits, source/typed identity and read-only failure must remain enforced.

## Implementation and reproduction

JVM lowering recursively validates literal alternatives before emitting the selector.
It flattens only the ordered literal tests: early matches jump to one shared hit
label, the final mismatch goes to the next case, and a false guard also goes to
that next case. Nested alternatives do not duplicate guards or bodies. Existing
initialization facts and normal/returning joins remain authoritative. Every
alternative descendant is charged against the expression node/depth limits.

The semantic pickler retains nested ALTERNATIVE structure, rather than flattening
source edges. It verifies canonical Int joins, each leaf's exact constant and type,
source/typed mappings and spans, arity, order and literal-only membership. Bindings
and wildcards inside alternatives are explicit errors. Tests corrupt child order,
identity, values, types, spans, cycles and widths and check read-only rejection.
The legacy method plan also exercises explicit expression-depth limits.

`tests/jvm_tasty_alternative_matches.rs` checks exact `(IJJ)J` descriptors,
branch instructions, local-slot counts and full stack-map frames for Long joins
and all-returning matches, nested wire structure, independent fresh-session
output, existing binder scope and unsupported inputs. The direct JVM source test
also executes the fixture independently of the semantic pickler.

```sh
SCALA39_ROOT=/path/to/pinned/scala3 JAVA_HOME=/path/to/jdk21 \
COURSIER_MAVEN_ROOT=/path/to/cached/maven \
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
cargo test --test jvm_tasty_alternative_matches --locked -- --nocapture --test-threads=1
```

Put the JDK's `bin` directory on PATH as well. The required JDK workflow includes
this target. The strict TASTy manifest and aggregate inventory also require it;
ordinary Rust tests may skip external consumers unless the flags above are set.

The later [Boolean-match contract](jvm-tasty-boolean-matches.md) extends the
selector/pattern family while preserving ordered cases and the final fallback.
