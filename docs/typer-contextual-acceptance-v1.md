# Acceptance audit of contextual argument typing, v1

**The source typer meets the v1 acceptance criteria.** All eleven selected
sites agree with Scala 3.9.0, including the context bound and implicit default
in the upstream regression `i8491.scala`.

## What the audit measures

The sample consists of eleven sites chosen by feature from the Scala compiler's
positive, negative, and run suites before inspecting the Rust outcomes.
It covers ordinary and inherited lookup, legacy implicits, companion search,
by-name recursion, ambiguity, and interactions between clauses. The denominator
is the selected sites. It gives no estimate of coverage across the Scala corpus.

The reference is Scala **3.9.0**, repository revision
`777528f19a58e794c9954a42f433373472ec57f8`. The runner verifies the revision,
the bytes of every selected file against its Git blob, the compiler artifact
hashes, and the compiler and JDK versions. Each upstream source gets a separate
Scala invocation and output directory. Scala accepts nine files and rejects
two with E172. Both sides see the same Scala runtime
jars. The audit checks typer output for run-suite inputs without executing
those programs. [Pinned Scala source tree](https://github.com/scala/scala3/tree/777528f19a58e794c9954a42f433373472ec57f8/tests)

Rust parses the entire source and runs the namer with the real filename.
After naming succeeds, each probe types the designated method body, initializer,
or statement using its original context. Positive observations compare selected
declarations, arguments or recursive references. Negative observations compare
the reason for rejection at the designated site. A rejection counts as a match when the typer reports the expected failure at the original call.

## Results on original sources

Paths below are relative to the pinned Scala repository. Focused reductions in
the last column help locate a failure and carry no additional acceptance votes.

| Source and site | Scala 3.9 result | Rust observation |
| --- | --- | --- |
| `tests/pos/test-implicits1.scala` · `Implicits.y` | `Implicits.foo(1)(Implicits.impl)` | **Match.** Both method and evidence resolve to the source declarations in `Implicits`. |
| `tests/pos/test-implicits.scala` · last expression in `Test.foo` | `implicitly[String](x)` | **Match.** The helper is the binary `Predef.implicitly` and the argument is the method-local implicit value. |
| `tests/pos/byname-implicits-2.scala` · `implicitly[Foo[Int]]` | `Foo.fooInt` | **Match.** The evidence points to the companion's source value and the application returns `Foo[Int]`. |
| `tests/pos/byname-implicits-1.scala` · `implicitly[Foo]` | A lazy dictionary entry calls `Foo.foo` with itself. | **Match.** The provider call references the source method. Its recursive argument and the final result refer to the same synthetic field. |
| `tests/pos/given-priority.scala` · first `summon[A]` | Selects `NormalImplicits.g2` after `import NormalImplicits.given`. | **Match.** The original import and initializer resolve to the source symbol for `g2` in both language modes. Typing the entire method subsequently encounters a separate `DuplicateLocalValue` on the second wildcard value declaration. |
| `tests/neg/byname-implicits-11.scala` · `implicitly[Foo[Int]]` | E172, missing evidence for `Foo[Int]`. The failed provider would require `=> Foo[Foo[Int]]`. | **Match.** Rust reports `MissingContextualArgument` at the original call for the exact `Foo[Int]` goal. Scala also prints the failed nested dependency. |
| `tests/run/implicit-disambiguation.scala` · `M.f` | The inner `a` supplies `summon[A].show`. Its initializer uses the parameter of type `B`. | **Match.** The initializer of the local `a` refers to the method parameter of type `B`. In the complete method body, `summon[A]` resolves to that same `a`, and the selection resolves to `A.show` with result `String`. |
| `tests/neg/given-ambiguous-2.scala` · RHS of top-level `f` | E172 names the conflicting `a1` and `a2` at the call of `g`. | **Match.** Typing the original method reports ambiguity at its call to `g` and identifies the source declarations `a1` and `a2`. |
| `tests/pos/i5295.scala` · top-level `foo` | The inline accessor for `bar` applies the context function with explicit argument `4`. | **Match.** In the original inline method body, Rust resolves the source `bar` and `ContextFunction1.apply`, keeps the explicit argument `4` and produces the declared `String` result. The comparison covers the source application before inline-accessor lowering. |
| `tests/run/curried-using-threading.scala` · `narrow()` | The call and its ordinary default getter receive the same contextual argument. | **Match.** Rust resolves the constructor-form given to `given_Context`. The call of `narrow` and the call of `narrow$default$2` reuse the same evidence. The getter body calls `root` with its own copy of the contextual parameter. |
| `tests/pos/i8491.scala` · `Test.qux2` | The synthesized witness precedes `B` in the implicit clause and the getter is `qux2$default$2`. | **Match.** The complete definition returns its `B` parameter. The polymorphic getter takes only the type parameter. Its default constructs a fresh anonymous class owned by the getter, with parents `Object` and `Bar`, and widens the result to `Bar`. |

The contextual CI job separately compares source-object exports on the shared
`ExportLibrary.scala`, `ExportClient.scala` and `ExportErrors.scala` fixtures.
Scala compiles the library before typing the clients. Rust names the combined
source unit and checks the selected method and original delegate identities,
the method-owned type and evidence parameters, and rollback on rejected calls.
The eight positive client sites and six negative sites are project controls
outside the eleven-site upstream sample.

Three additional project fixtures exercise chained exports. `ChainedExports.scala`
has nine successful sites, including the one-hop control, direct selection,
search inside the exporting object and a polymorphic provider with its own
evidence. `ChainedErrors.scala` checks seven rejections. `ChainedCycle.scala`
checks the missing witness on a cycle of two empty exports. The Rust tests also
check declaration order, precompleted modules, rollback and retry. The existing
contextual CI job runs both the Rust tests and pinned Scala controls.

The audit also runs three project controls for an ordinary `class Box[A: TC]`.
The positive control checks the private witness used by `summon[TC[A]]` in
the class and the caller's witness passed to `new Box[Int]`. Two negative
controls check missing and ambiguous constructor evidence on the same source
in each compiler. Three corresponding case-class controls check inference at
`Box(1)`, forwarding from the generated companion `apply` to the primary
constructor, and the two evidence errors at the factory call. Three copy
controls compare the receiver’s default field, the new witness passed from
the caller through `copy` to the constructor, and the missing and ambiguous
evidence diagnostics at `box.copy()`.

For `case class Envelope[A: TC](value: A)(using trace: Trace)`, the audit
checks the argument order at direct construction and generated `apply` and
`copy` calls. The copy reads the original field and forwards both witnesses
from the copy site. Twelve negative sites cover missing and ambiguous
`TC[Int]` and `Trace` independently at each of the three entry points.

The existing contextual argument and consumer fixtures provide additional
controls for structural givens, dependent argument paths, contextual defaults,
nested context functions, conversions, extensions, extractors, and binary
providers. They exercise different samples from the eleven sites above.
The [typer compatibility contract](typer-v0.1-compatibility.md#contextual-arguments-in-scala-39)
describes their coverage and the separate output contracts for JVM and TASTy.

## Scope of closure

The eleven in-scope observations and the focused regressions cover source typing.
The curried test exercises the original `narrow()` inside `Test`, as well as
the definition of `narrow` and its default expression. The run-suite program's
`@main` launcher, JVM output and TASTy emission have separate acceptance criteria.
The `i8491.scala` observation covers source typing of the default body. Emission
of its anonymous class has a separate output contract.

## Reproduction

The [CI workflow](../.github/workflows/typer-contextual-v1.yml) runs the audit
with `--require-parity` on pull requests targeting `main` and pushes to
`main` that change the frontend or its audit dependencies. It can also be
started manually in GitHub Actions. The same job also compares the contextual
consumer fixtures with the pinned compiler, including extension overload groups
in both declaration orders, explicit `using` and inherited overloads. The
contextual argument fixtures check binary defaults by rebuilding the library,
comparing its TASTy bytes, and compiling separate positive and negative clients.
The multiclause clients cover partial and full omission, an intervening empty
clause, a final witness search, and legacy implicit defaults with curried
getters. Rust tests check the identity and evaluation order of the typed operands. These
controls are reported separately from the eleven v1 observations.

Use JDK 21, a checkout of the revision above and the cached artifacts specified
by the [oracle setup](../tools/tasty-pickler-oracle/README.md). Export
`JAVA_HOME`, `SCALA39_ROOT`, and `COURSIER_MAVEN_ROOT`. Run the following commands from the workspace root.

```sh
python3 crates/dotty-typer/tests/fixtures/contextual-v1/check.py --require-parity
python3 crates/dotty-typer/tests/fixtures/contextual-arguments/check.py
python3 crates/dotty-typer/tests/fixtures/contextual-consumers/check.py
```

The first command prints the result per upstream site. With `--require-parity`
it exits 1 while any in-scope site is blocked or disagrees with Scala.
Omit the flag to verify the recorded observations, including known failures.
Both modes fail if a source differs from the pin, Scala output misses an expected observation, or a Rust assertion fails.
When fixing a gap, change the corresponding probe to assert the Scala behavior
and record `match` after that assertion. The Rust probes require the pinned
external inputs, so the ordinary workspace test run lists them as ignored.
