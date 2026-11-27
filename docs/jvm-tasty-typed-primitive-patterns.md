# Same-type primitive patterns in JVM and TASTy

Reference: Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Base main:
`38e2257cc198fe02a1562342b5bed597c8702977`.

## Scope and increments

1. Freeze this contract and the pinned reference with independent Java/Scala clients.
2. Lower proved Int/Boolean typed wildcard and binder patterns in JVM.
3. Authenticate their exact source/typed shapes and retain pattern TYPED in TASTy.
4. Cover composition, malformed plans, limits, effects, scopes and determinism.
5. Run real Rust/JDK/Scala/from-TASTy consumers and repository checks; update gates/docs.

For an existing Int or Boolean selector, accept the same unqualified primitive
pattern type: `case _: Int`, `case n: Int`, `case n @ (_: Int)` and Boolean
counterparts, including existing parentheses. Selector families include formals,
locals, case binders and literal singletons. Results retain the existing
Int/Boolean/Unit/Long/Float/Double subset. Existing literal prefixes, guards,
shadowing, nested matches, locals, assignments, expression ascriptions and local
returns compose within their current supported positions.

The selector runs once; a reached guard runs once. A false guard skips the whole
case and restores binder visibility/initialization. A proved typed wildcard or
binder is irrefutable only without a guard and must be the final case. Guard-only
matches still need a fallback, including a literal-true guard. Existing exhaustive
Boolean literal coverage is unchanged.

Excluded: reference/Any selectors, runtime type tests, casts, boxing, subtype or
widening patterns, qualified/aliased/generic syntax, Long/Float/Double selectors,
typed patterns or binders inside alternatives, nested binders, extractors, partial
matches/MatchError lowering and general reachability. Returns inside unsupported
value or guard positions remain excluded.

## Identity and producer contract

The JVM validates the wrapper, reified type tree and wildcard against the exact
canonical primitive type after normalizing the selector family. It then reuses
the existing irrefutable-case path; no runtime test or conversion is needed.

TASTy retains original pattern TYPED nodes and primitive syntax. The implicit
source `Typed(Ident(n), tpt)` becomes typed `Bind(n, Typed(Ident(_), typed_tpt))`.
Both the source Typed and source identifier bind to the typed Bind. The synthetic
Typed and wildcard have no invented source mappings. Explicit `n @ (_: Int)`
uses the ordinary Bind and inner Typed mappings. Validation must authenticate
these different edges, declaration/scope identity and source syntax, rather than
reuse the expression-ascription operand validator.

Both output paths borrow the canonical session stores and exact roots. Preserve
read-only rejection, item/depth/node limits, deterministic output and failure
without partial artifacts. No frontend, shared AST or codec redesign is needed.

## Reference and acceptance

The 45-method fixture has 56 pattern TYPED nodes, 37 binders, 47 matches and four
returns. Scala additionally inserts expression widening wrappers. Rust preserves
its own canonical trees rather than copying Scala's synthetic nodes. Scala's
same-type primitive pattern bytecode retains a constant-true test and defensive
MatchError path; equivalent Rust lowering may use the proved irrefutable path.
The contract is semantic/runtime compatibility, not instruction equality.

Each independent client performs 576 checks, including Int extremes, both Boolean
values, signed floating zeros and non-finite numeric results, selector/guard/body
effect order, false guards, nested binders and returns. The pinned reference
runner records hashes and executes original and separately rebuilt from-TASTy
classes. Actual Rust-produced pairs must pass the same four consumer runs, with
original hashes unchanged. Scala-generated reference evidence alone does not
establish Rust interoperability.

Tests extend the existing mandatory `jvm_tasty_guarded_matches` target and direct
source-to-JVM target. Run with JDK 21 on PATH and:

```sh
SCALA39_ROOT=/path/to/pinned/scala3 JAVA_HOME=/path/to/jdk21 \
COURSIER_MAVEN_ROOT=/path/to/cached/maven \
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
cargo test --test jvm_tasty_guarded_matches --locked -- --nocapture --test-threads=1
```

## Implemented verification

The existing guarded-match target now runs 22 tests, including 11 typed-pattern
regressions. With mandatory pinned Scala/JDK consumers and serial libtest
execution it measured 12.31 seconds locally. The strict inventory remains at 38
targets with unchanged assignments; shard 3 includes the measured increase.

JVM tests pin the selector's single store and lack of runtime test instructions,
reject malformed canonical types, type trees, wildcard shapes and cycles, and
charge every pattern child against depth/node limits. A separate direct
source-to-JVM test passes all 576 Java checks without TASTy production.

Paired tests preserve 56 pattern TYPED nodes plus two expression ascriptions,
37 distinct BIND declarations, 47 MATCH nodes and four RETURN nodes. They check
fresh-session determinism, exact source edges (including implicit binders),
source spans/syntax, read-only failures, validation/projection limits and
unsupported shapes. Existing scope/evidence regressions now run against both
plain and typed binders. Guarded Boolean typed binders with Long/Float/Double
results have exact descriptors, instructions, physical slots and full stack-map
frames, plus JDK execution with extrema and signed zeros.

Independent Java and Scala clients each pass 576 checks on original Rust output
and separately rebuilt from-TASTy classes. The original class/TASTy hashes are
unchanged. This evidence is separate from the Scala-generated reference.

The complete local workspace check passed 8649 tests (57 existing ignored) and
44 all-feature doctests. Formatting, frontend boundaries, Clippy/rustdoc with
warnings denied, the standalone compiler client, strict inventory/timing tests,
workflow lint and diff checks also passed.
