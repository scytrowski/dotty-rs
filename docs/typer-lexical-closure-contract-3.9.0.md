# Typer lexical closure capture contract (Scala 3.9.0)

This document freezes the source-Typer handoff for ordinary `Function1`
lambdas before the capture mechanisms in issues #1861–#1864. It records Scala
3.9.0 typer evidence and the Rust full-method baseline at
`7f504378974920991ca6779480b40c83f8c904d5`. Rust acceptance here is a baseline
receipt; no outer-value or explicit-`this` capture is enabled by this change.

## Reproduction and provenance

The Scala source checkout is pinned to
`777528f19a58e794c9954a42f433373472ec57f8`. Oracle sources, full normalized
`-Vprint:typer` snapshots, the SHA-256 manifest for all eight runtime jars, and
the runner are in
[`crates/dotty-typer/tests/fixtures/lexical-closure-contract`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract).
The runner verifies the checkout revision, JDK release and every artifact hash
before compiling. It compiles each positive source independently and checks
wrong-result and unknown-reference diagnostics.

The runner pins Eclipse Temurin JDK `21.0.8+9` (`JAVA_VERSION="21.0.8"`) and
Scala compiler/library artifacts `3.9.0`. Reproduce from the repository root:

```sh
SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8 \
JAVA_HOME=/path/to/jdk-21 SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/coursier/https/repo1.maven.org/maven2 \
crates/dotty-typer/tests/fixtures/lexical-closure-contract/run
```

The script's default scratch path is the worktree-local `target/` directory;
`DOTTY_LEXICAL_CLOSURE_TMP_ROOT` can point it elsewhere. Snapshots are from the
compiler `typer` phase, before lambda lifting. They are semantic evidence for
types, lexical references, shadowing, and receiver meaning, not a required
byte-for-byte encoding for this Typer's `Closure` node.

## Pinned source outcomes

| Fixture | Scala 3.9.0 result | Expected semantic relationship |
| --- | --- | --- |
| `Positive.scala`: `identity` | Accepted, `Int => Int` | Lambda has no lexical captures. |
| `parameter(delta)`: `x + delta + delta` | Accepted, `Int => Int` | Both reads refer to the enclosing method parameter `delta`; one declaration identity is captured once, at first use. |
| `local`: immutable initialized `delta` | Accepted, `Int => Int` | Read refers to the local `val` owned by the enclosing method body. |
| `shadow(delta)`: `(delta: Int) => delta` | Accepted, `Int => Int` | The body refers to the lambda parameter, not the enclosing method parameter. |
| `C.explicitThis`: `_ => this` | Accepted, `Int => C` | `this` denotes the enclosing `C` instance. |
| `C.explicitMember`: `this.base` | Accepted, `Int => Int` | Selected member lookup uses the same enclosing `C` instance. |
| `C.implicitMember`: `base` | Accepted, `Int => Int` | Unqualified member lookup uses that same enclosing instance. |
| `Mutable.scala`: mutable local `delta` | Accepted, `Int => Int` | Scala's typer resolves the read to the mutable local; Rust var boxing/capture is deferred. |
| `WrongResult.scala` | Rejected | `Int` body result does not conform to expected `Boolean`. |
| `UnknownReference.scala` | Rejected | `missing` has no lexical declaration. |

The full snapshots are kept alongside the source fixtures. In particular, the
Scala typer represents `this.base` and unqualified `base` with the enclosing
`C` receiver. Later lambda-lift output may introduce a different tree shape;
that is not the contract for source Typer.

## Capture identity and ownership contract

A future supported capture is identified by the canonical captured
`SymbolId`, together with its lexical owner and capture kind. Name text is
never sufficient to establish capture identity. Capture kinds in this bounded
contract are enclosing method parameter, initialized immutable local, and
lexically enclosing instance. Mutable locals remain explicitly deferred.

For a typed lambda:

- Resolve every body reference against the canonical declaration visible at
  that source use. An inner lambda parameter shadows an outer declaration even
  when both have the same spelling.
- Preserve the source-use type of each captured reference. Do not rebuild a
  capture from its name, allocation order, or a guessed type.
- `Closure.env` is the shared typed-node representation of the environment.
  Each environment entry must trace to the canonical capture identity and its
  typed reference; this contract does not add a parallel capture structure.
- Order environment entries by first lexical use and deduplicate repeated
  references to the same canonical declaration. Repeated uses do not create
  multiple captures.
- The current source Typer owns a synthetic lambda method by the enclosing
  source method (`ExpressionContext.owner`). Its generated parameters are
  owned by that synthetic lambda method. Captured declarations retain their
  original owners; capture does not reparent them. Future capture receipts
  must preserve both ownership edges.
- Explicit `this` and implicit unqualified instance-member access have the
  same enclosing-instance identity when they refer to the same class/trait
  instance. Package members, object/module members, and static-like terms are
  not instance captures merely because lookup is unqualified.
- Every successfully typed source method is a real typed `DefDef`. The exact
  method root and each visited source lambda/body have the matching
  `SourceTypedIndex` entries. The closure's synthetic method identity is
  retrievable from `FunctionLiteralIndex` for its source lambda.
- Failure rolls back typed nodes, source mappings, and lambda identities as a
  unit. Retrying the same method produces the same first error without stale
  closure mappings. Arena IDs are session-local; tests prove relationships
  inside each session and never compare same-slot IDs across sessions.

`Closure.env` entries are typed trees, while `FunctionLiteralIndex` tracks the
synthetic method, scope, typed definition, and source parameter identity. The
baseline tests assert that the synthetic method's direct owner is the source
method and that the lambda parameter's direct owner is the synthetic method.
These existing shared structures are sufficient for this handoff; no new core
capture representation is introduced.

## Rust full-method acceptance baseline

`lexical_closure_contract_records_whole_method_baseline` in
`crates/dotty-typer/src/typer/tests/lexical_closure_contract.rs` consumes the
named `RustBaseline.scala` fixture and invokes
`SourceTyper::type_method_definition` on each method in one source unit.
It verifies the typed root and exact source mapping on success, complete method
symbol information for supported functions, canonical shadowing identity,
first-error source location for the enclosing parameter, retry behavior, and
absence of lambda/method mappings after failure. These synthetic positive
fixtures are separate from any standard-library cohort denominator.

Observed at frozen Rust main SHA `7f504378974920991ca6779480b40c83f8c904d5`:

| Rust source method | Full `DefDef` typed? | First observed outcome |
| --- | --- | --- |
| `identity` | Yes | Typed `DefDef`; RHS maps to a `Closure` with an empty environment; lambda identity is indexed. |
| `shadow` | Yes | Body's canonical `TermRef` targets the lambda parameter owned by the synthetic lambda method, not the enclosing parameter; the synthetic lambda method is owned by the source method. |
| `parameter` | No | `FunctionLiteralCaptureUnsupported` at the first `delta` read. Error symbol matches the source method parameter; method/lambda/body mappings are absent, and retry yields the same capture error. |
| `local` | No | `FunctionLiteralCaptureUnsupported` at the immutable local read; failed method/lambda mappings are absent. |
| `mutable` | No | With an explicitly typed lambda parameter, `FunctionLiteralCaptureUnsupported` is reached for the mutable local read. Mutable capture remains deferred. |
| `explicitThis` | No | `FunctionLiteralCaptureUnsupported` at `this`, with no selected symbol. |
| `implicitMember` | Yes | Typed `DefDef` and `Closure`; the current closure environment is empty. This is an observed Typer receipt, not evidence of a serialized environment or a compose/andThen success claim. |
| `wrong` | No | `ExpectedExpressionTypeMismatch` (or the narrow conformance-unsupported variant), because the body result does not conform to the declared `Function1` result. |
| `unknown` | No | `TermNameNotFound`; lookup does not invent an enclosing capture. |

The acceptance assertions preserve exact source ownership and spans rather than
hard-coding arena numbers. The enclosing parameter case verifies that the
reported symbol is the source parameter's canonical symbol. Capture failures
leave no typed method root, closure mapping, or `FunctionLiteralIndex` entry.
The local and mutable `symbol` payloads are intentionally not dereferenced
following rollback: transaction-created IDs may not remain valid after the
failure. The contract requires future successful captures to use canonical
session symbols; it does not claim post-rollback validity for those error
payloads.

The Rust method `implicitMember` currently types with an empty `Closure.env`.
That baseline is kept visible because current success at the source-Typer
boundary does not settle later lowering or receiver-capture encoding. Explicit
`this` remains rejected. Neither result establishes `Function1.compose` or
`andThen` success.

## Deferred acceptance shapes

The Scala oracle is positive for mutable capture; the Rust implementation and
its capture policy defer mutable `var` capture. Future capture issues should
extend the named whole-method harness with success receipts for their assigned
capture kinds, including exact canonical declaration/owner/type proof, stable
first-use environment order, repeated-reference deduplication, and rollback.
The current negative controls (`wrong`, `unknown`) remain required. Invalid
owner construction and cross-session capture reuse are guarded by the
single-session identity rule: do not authenticate foreign IDs by arena slot or
spelling. Shadowing is already asserted positively by the baseline test. Two explicitly
ignored Rust placeholders are also checked in: future successful captures must
preserve first-use order/deduplicate repeated references, and capture support
must reject a canonical source symbol whose owner has been changed to an
unrelated lexical owner. They remain ignored so required CI stays green until
the downstream capture issues make their success/rejection proofs meaningful.
