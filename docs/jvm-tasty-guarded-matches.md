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
