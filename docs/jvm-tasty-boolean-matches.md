# Bounded Boolean-selector matches in JVM and TASTy

Target Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main: `c1fdabb6b9908b61addbc609e5fcc71b0ec6fd3d`.

## Scope and increments

1. Freeze this contract, the pinned Scala oracle and independent consumers.
2. Validate and lower Boolean literal/wildcard matches in JVM methods.
3. Preserve canonical Boolean binders, guards and literal alternatives in JVM.
4. Validate source/typed identities and emit complete semantic TASTy, including
   paired agreement on Boolean pattern-local proofs.
5. Require independent Java/Scala consumers, original and from-TASTy execution,
   malformed-plan, limit and determinism coverage and CI gates.

This extends the existing Int matcher with Boolean selectors. Selectors include
Boolean literals, formals, visible case binders and the existing supported
Boolean condition expressions. Cases accept `true`, `false`, wildcards, plain
immutable bindings, explicit bindings over literals/wildcards, and literal-only
alternatives (including nested parentheses and duplicates). Exactly one final
unguarded irrefutable case is required, even for `true | false`. Cases retain
source order; the selector is evaluated once and a guard only after a pattern
hit. A false guard skips the entire case, including its remaining alternatives.

Results retain the existing Int, Boolean, Unit, Long, Float and Double subset,
including local returns and nested Boolean/Int matches. Bindings have distinct
canonical symbols/scopes even with repeated spelling or parameter shadowing.
A constant selector can give a binder a Boolean singleton type; retain that
information in TASTy rather than silently widening it.

Ordinary Boolean ValDef locals, their assignments, arbitrary Boolean calls and
general effectful Boolean blocks remain separate unsupported producer shapes.
Existing Int-returning calls inside comparisons provide observable selector and
guard effects without extending that scope. Exhaustiveness analysis without a
final irrefutable case, MatchError lowering, non-Int/Boolean selectors, typed
or extractor patterns, bindings/wildcards within alternatives, reference results,
new numeric operations and semantic method-body unpickling are excluded.

## Evidence and acceptance

The adjacent `tests/fixtures/jvm_tasty_boolean_matches/run.py` reuses the pinned
offline oracle toolchain, including revision and compiler artifact SHA checks.
No-argument mode generates Scala reference evidence. Candidate mode must read
actual Rust-produced classes/TASTy and execute independent Java/Scala consumers,
both against original artifacts and separately rebuilt `-from-tasty` classes.
Original artifacts must remain unchanged. This establishes semantic/runtime
compatibility, not byte equality with Scala output.

Both producers must authenticate canonical Boolean types despite JVM Int and
Boolean sharing the Integer verification category. Validate exact source/typed
edges, constants, declaration/reference identities and case scope; reject
foreign same-spelling bindings, cross-case reads, forged types, cycles and
resource-limit violations read-only, with no partially published artifacts.
Keep existing dependency boundaries and use the existing match machinery.

```sh
SCALA39_ROOT=/path/to/pinned/scala3 JAVA_HOME=/path/to/jdk21 \
COURSIER_MAVEN_ROOT=/path/to/cached/maven \
python3 tests/fixtures/jvm_tasty_boolean_matches/run.py
```

Put the JDK's bin directory on PATH as well. Final implementation acceptance
requires the complete repository checks, mandatory JDK gate and all strict
TASTy shards, follow-ups and aggregate with the new target registered once.

## Implemented behavior and validation

Both producers now use the same selector family for literals, wildcard/binder
types and alternative joins. JVM lowering reuses the existing one-slot selector
temporary and ordered branches. It retains exact Z descriptors and Boolean
semantic kinds while emitting int-like loads/stores/tests. Normal joins and
local returns preserve existing numeric value widths and frame tags.

The pickler authenticates Boolean binder reads only after their original case
proof has been checked; ordinary Boolean declarations are still rejected.
Singleton pattern types use TRUEconst/FALSEconst rather than general type
projection. The paired ABI contract now compares Boolean pattern proofs as
well as the existing Int local evidence, including exact case/scope identity.
Neither frontend representations nor production dependencies were changed.

`tests/jvm_tasty_boolean_matches.rs` covers source acceptance, exact numeric
join descriptors/opcodes/full stack-map frames, literal and nested alternative
wire order, singleton pattern types and distinct same-name BIND/reference
addresses. Corrupted selectors, literals, wildcard types, guards, case proofs,
foreign-case reads, forged Nothing joins, repeated/cyclic/oversized alternatives,
validation/encoding depth limits, exclusions and fresh-session determinism have
independent regressions. The direct source-to-JVM fixture also runs separately
from TASTy production in `tests/jvm_source_compilation.rs`.

The independent Java client verifies both selector values, all guard paths,
skipped guards on pattern mismatch, exactly one selector/guard evaluation,
false-guard continuation after duplicate alternatives, case shadowing/nesting,
Long bounds, Float/Double signed zero/subnormals and normal/mixed/all-returning
paths. The Scala client consumes original and rebuilt artifacts as well. These
checks run with `java -Xverify:all -ea` and leave original Rust files unchanged.

```sh
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 cargo test --test jvm_tasty_boolean_matches --locked -- --nocapture --test-threads=1
```

Use the pinned Scala/JDK environment above. The final local serial target took
6.65 seconds with all 17 tests passing. The target is required by the JDK
workflow and strict TASTy shard 2, with the independent inventory increased to
36 targets. All four shards, their aggregate and the mandatory follow-ups remain
required; existing target measurements retain their recorded provenance.

Final local acceptance used Rust 1.98.1 and Temurin 21.0.13. Workspace all-target
checks passed 8,581 tests (57 existing ignored); all-feature doctests passed 44
examples. Format, frontend dependency boundaries, Clippy/rustdoc with warnings
denied, the standalone compiler client, manifest/timing tests, workflow lint and
diff checks passed. All 36 strict targets, four mandatory follow-ups and the
aggregate passed, including the JDK gate with external checks required.
