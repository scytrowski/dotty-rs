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
the proposed JVM proof closes unreachable failure on the canonical Boolean domain
without adding general exception emission.

```sh
python3 tests/fixtures/jvm_tasty_boolean_exhaustive_matches/run.py
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 cargo test --test jvm_tasty_boolean_exhaustive_matches --locked -- --nocapture --test-threads=1
```

Set `SCALA39_ROOT`, `COURSIER_MAVEN_ROOT` and `JAVA_HOME` as documented by the shared
oracle; put the JDK bin directory on PATH. Final acceptance includes all repository
checks, mandatory JDK execution, strict TASTy shards/follow-ups and their aggregate.
