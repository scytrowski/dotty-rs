# Bounded exhaustive Boolean matches in JVM and TASTy

Reference Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0 and class-file major 65. Base main: `34d5c494cf8528a7fff2759aa67bd957b5fdea0a`.

## Scope and increments

1. Freeze this contract and the pinned Scala reference with independent clients.
2. Validate Boolean coverage and close the final JVM case failure edge safely.
3. Validate exact source/typed coverage and preserve original TASTy case structure.
4. Compose guards, alternatives, existing binders, locals, nesting and local returns.
5. Require actual Rust/original and from-TASTy execution, malformed-plan/limit and
   determinism regressions, current documentation and mandatory CI inventories.

A match with a canonical Boolean selector may omit its final wildcard when the
validated unguarded cases cover both `true` and `false` and the last case has no
guard. Preserve existing pattern and result subsets: Boolean literals, literal-only
alternatives (including parentheses and duplicates), existing binders over literals,
and Int/Boolean/Unit/Long/Float/Double bodies. Existing final wildcard/binder behavior
is retained. Early unguarded irrefutable cases remain rejected.

| Pattern | Guaranteed Boolean coverage without guard | With guard |
| --- | --- | --- |
| `true` / `n @ true` | true | none |
| `false` / `n @ false` | false | none |
| literal alternative | union of its literal values | none |
| wildcard / ordinary binder | both, with existing final-case rule | none |

Coverage follows authenticated constants and canonical selector identity, never
JVM Integer verification tags, spelling alone, or duplicate literal counts. Guards
never prove coverage, including literal `true` guards. A false guard skips the
whole case, including duplicate alternatives. The selector runs once; guards run
only on pattern hits and once per case. Keep exact binder symbols/scopes and
normal/diverging result joins. The final JVM failure path must be closed using the
coverage proof; weakening validation alone leaves an invalid missing-result path.

TASTy retains original MATCH/CASEDEF/literal/BIND/ALTERNATIVE nodes and case order.
Do not add a synthetic source wildcard, fake declaration or generalized type model.
Both producers borrow the same canonical session; validation is read-only, bounded
and rejects malformed proof/type/source edges without publishing partial artifacts.

Partial matches and MatchError lowering, guard-based or singleton-selector-only
exhaustiveness, final guarded suffixes without a fallback, Int exhaustiveness,
new pattern/selector/result families, binders/wildcards in alternatives, arbitrary
Boolean calls, new operators/conversions and returns in operand positions are
excluded. This increment does not implement general exhaustivity analysis.

## Evidence and acceptance

The adjacent fixture runner reuses the offline pinned pickler oracle. No-argument
mode generates **Scala-produced reference** evidence; candidate mode must consume
actual Rust artifacts, compile independent Java/Scala clients, rebuild Rust TASTy
with `-from-tasty`, execute both outputs with JDK 21 verification and check original
bytes unchanged. Semantic/runtime compatibility is required, not byte equality.
Pinned Scala retains a defensive MatchError branch even for these Boolean matches;
the JVM proof closes unreachable failure on the canonical Boolean domain
without adding general exception emission.

```sh
python3 tests/fixtures/jvm_tasty_boolean_exhaustive_matches/run.py
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 cargo test --test jvm_tasty_boolean_exhaustive_matches --locked -- --nocapture --test-threads=1
```

Set `SCALA39_ROOT`, `COURSIER_MAVEN_ROOT` and `JAVA_HOME` as documented by the shared
oracle; put the JDK bin directory on PATH. Final acceptance includes all repository
checks, mandatory JDK execution, strict TASTy shards/follow-ups and their aggregate.

## Implemented behavior and regression evidence

JVM validation collects finite Boolean coverage only after validating canonical
selector/pattern types and literal payloads. Its private case plan clears the
final literal tests only when complete unguarded coverage is proven, making that
case the residual default while retaining any binder. Existing temporary slots,
guards, initialization joins and return-flow validation remain in use.

The pickler computes irrefutability and finite Boolean coverage in one existing
budgeted source/typed pattern walk. Literal alternatives retain their original
children and duplicate values. Only unguarded cases contribute guaranteed coverage.
The existing encoder emits the unchanged case vector and exact BIND/reference
identities; no frontend, public API or wire representation change is required.

The 30-method fixture checks both literal orders, alternatives/duplicates, binders,
guarded prefixes, one selector/guard evaluation, ordered side effects, short-circuit
skips, mutable assignments and loops, nested Boolean/Int matches, normal/mixed/all
returning paths and primitive values including signed zeros, subnormals and Long
bounds. Independent Java and Scala clients consume original Rust artifacts and
Scala's separately rebuilt from-TASTy classes with verification enabled. Original
artifact hashes remain unchanged.

The dedicated target has 12 tests, including exact Z/I/J/F/D descriptors, load/store
and return instructions and complete stack-map frame offsets/locals/stacks; fresh
session byte determinism; read-only malformed literal/type/guard/join/case-edge and
foreign-binder rejection; cyclic/oversized alternatives; explicit method-only
validation and encoding budgets; and partial/guard-only/Int/singleton exclusions.
Lowest-layer pickler tests preserve literal order, nested duplicate alternatives
and reject partial or guarded-only coverage. A direct source-to-JVM fixture runs
independently of TASTy production.

The final serial Scala/JDK target measured 6.85 seconds locally. CI requires the
new target in the JDK job and strict TASTy shard 2; the independent inventory now
contains 38 targets. Historical target measurements and existing shard assignments
are preserved.

Local repository checks passed 8,618 workspace tests (57 existing ignored tests)
and 44 all-feature doctests using Rust 1.98.1 and Temurin 21.0.13. Formatting,
frontend dependency boundaries, Clippy/rustdoc with warnings denied, the standalone
compiler client, strict inventory/timing tests, workflow lint and diff checks
passed. Local workspace checks used a 65,536-file process limit for the existing
parallel JMOD tests; no classloader behavior or expectations changed.

All 38 strict TASTy targets passed across four local shards, with the constructor
alias, loader rejection, dual-reference reader and mandatory JVM verifier/execution
follow-ups passing as well. The strict aggregate passed. Initial shard runs exposed
two additional historical tests classifying these exhaustive Boolean shapes as
unsupported; those entries now exercise genuinely partial coverage, while the
supported shapes are independently covered by the new positive target. The affected
shards passed in full after that intentional expectation update.
