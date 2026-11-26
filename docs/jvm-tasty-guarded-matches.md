# Guarded Int matches and case-local bindings

Target Scala 3.9.0 revision `777528f19a58e794c9954a42f433373472ec57f8`,
TASTy 28.9.0, class-file major 65. Branch base is main `ff15e257ae604b467a887c72c9c0f27c1efe7725`.

## Scope and increments

1. Freeze the pinned Scala oracle and producer contract.
2. Emit Boolean guards on literal and wildcard Int cases in both producers.
3. Retain canonical pattern bindings and lower them in JVM methods.
4. Emit BIND declarations and references in TASTy; validate paired identities.
5. Require independent Java/Scala consumers and execution after `-from-tasty`.

The selector is evaluated once. Cases remain ordered. A guard executes only
after its pattern matches, and a false guard continues with the next case.
Case binders are immutable Int locals visible only to that case's guard and
body, including nested matches. Repeated spelling in distinct cases does not
merge symbol identities. Supported roots are Int literals, wildcards, plain
variable patterns, and explicit bindings over Int literals or wildcards.
Exactly one final unguarded irrefutable case is required. Results remain
Int, Boolean or Unit. Extractors, alternatives, runtime type tests, reference
selectors and non-exhaustive matches remain outside this increment.

The adjacent `tests/fixtures/jvm_tasty_guarded_matches/run.py` reuses the
pinned offline oracle toolchain. Scala-generated output is reference evidence;
producer acceptance must separately consume genuine Rust artifacts. CASEDEF
wire order is pattern, body, optional guard. BIND declares the canonical
symbol; TERMREFdirect must address its declaration, including when two cases
use the same printed name.

## Canonical input and implementation

The complete class-state/paired producer consumes pattern proofs through
`SourceTypedBindings::pattern_value_bindings`. The older literal-oriented
`PicklePlan` path does not consume these case-binding proofs. A caller can use
`ClassStatePicklePlan` for an ordinary method-only class as well as a stateful
class. Both producers borrow the original session's store and typed arena.

`SourceTyper::typed_pattern_binding_at` exposes the exact source/typed/symbol/
scope tuple. `into_typed_output` preserves these tuples separately in
`TypedSourceOutput::pattern_bindings`; it does not classify a Bind as a ValDef.
The method and class planners retain them when the typer is released or another
source extends the arena. An implicit variable source root maps to a typed
Bind with a synthetic wildcard child; no source mapping is invented for that
child. Explicit `n @ _` and `n @ 0` retain their genuine child mappings.

`JvmLocalBinding::pattern` records the canonical case scope and typed CaseDef
along with the Bind and symbol. Validation checks the exact case-pattern edge,
method containment, symbol identity, immutable flags and scope membership.
The local layout preserves the case identity, and lowering checks it again for
the case currently being emitted. The selector uses one temporary Int slot;
binder slots come from the deterministic method layout and cannot overlap it.
Nested matches reserve distinct selector temporaries. Initialization facts for
the binder are removed before false-guard continuation and the match join.
Ordinary initialization facts propagate along guard-false edges and are
intersected with literal-mismatch edges and normally completing bodies.

The pickler validates each source/typed pattern edge and the binding tuple,
then exposes that symbol only while checking its case guard and body. Outer
locals/binders remain visible through nested matches. Missing, duplicate,
stale and cross-case tuples are typed errors; no session mutation or partial
artifact pair escapes failure. Pattern proofs are indexed once for bounded
lookup rather than copied for every case. Existing item, depth and expanded
expression limits still apply.

CASEDEF wire order remains pattern, body, optional guard. BIND declarations
use reserved labels, and TERMREFdirect addresses that declaration even when
multiple cases share a name. A literal selector can give its binder a singleton
Int type; the wire pattern type retains that constant rather than silently
widening it. Guard expressions use the existing bounded Boolean condition
subset; this does not enable arbitrary Boolean calls, extractor lowering or
runtime type casts.

## Acceptance and reproduction

`tests/jvm_tasty_guarded_matches.rs` exercises exact case/guard structure,
independent case symbols/scopes, explicit and implicit bindings, nested cases,
public method planning after typer release, read-only rejection of corrupted
proofs, invalid guards and foreign-case reads, unsupported pattern/escape
rejection, and byte-for-byte determinism in fresh sessions.

The fixture covers Int boundaries, negative literal bindings, a constant
selector, parameter shadowing, Boolean and Unit results, short-circuit guards,
one selector evaluation, skipped guards on literal mismatch, false-guard
effects and nested references. Independent Java and Scala clients consume the
original Rust pair. The strict runner also uses the pinned Scala compiler's
reader and `-from-tasty`, then executes rebuilt classes with the Java client.
Consumers write separate output and leave the original artifacts unchanged.
These are semantic/runtime interoperability checks, not byte equality with
Scala-produced output.

```sh
SCALA39_ROOT=/path/to/pinned/scala3 JAVA_HOME=/path/to/jdk21 \
COURSIER_MAVEN_ROOT=/path/to/cached/maven \
DOTTY_CLASSFILE_REQUIRE_JDK=1 DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
cargo test --test jvm_tasty_guarded_matches --locked -- --nocapture --test-threads=1
```

With the external gates required, all 11 tests pass. The target is included in
the mandatory JDK workflow and in strict Scala shard 3; all four shards and the
aggregate/follow-up gates remain required. No full hosted Scala corpus is added.
