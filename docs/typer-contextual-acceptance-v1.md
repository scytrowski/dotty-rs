# Acceptance audit of contextual argument typing, v1

**Keep the typer milestone open.** Of 11 selected upstream sites,
five agree on the chosen evidence, one exposes an in-scope search discrepancy,
four stop before the intended observation, and one exercises a previously
documented unsupported combination. Growing by-name search still prevents closure. The original sites also expose three frontend prerequisites.

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
the reason for rejection at the designated site. Matching compiler exit status
alone would conceal the remaining search discrepancy.

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
| `tests/neg/byname-implicits-11.scala` · `implicitly[Foo[Int]]` | E172, missing evidence for `Foo[Int]`. The failed provider would require `=> Foo[Foo[Int]]`. | **Typer gap.** Rust rejects the same call with `ContextualSearchLimit`. The diagnostic exposes the implementation limit. Scala identifies the missing nested dependency. |
| `tests/run/implicit-disambiguation.scala` · `M.f` | The inner `a` supplies `summon[A].show`. Its initializer uses the parameter of type `B`. | **Blocked at member selection.** Rust reports `UnstableSelectionPrefix` on `.show`. Returning just `summon[A]` confirms both symbol choices in the original lexical scope. |
| `tests/neg/given-ambiguous-2.scala` · RHS of top-level `f` | E172 names the conflicting `a1` and `a2` at the call of `g`. | **Blocked at owner lookup.** Rust lacks class information for the synthetic file owner. Moving the declarations into an object reproduces the expected ambiguity in both compilers. |
| `tests/pos/i5295.scala` · top-level `foo` | The inline accessor for `bar` applies the context function with explicit argument `4`. | **Blocked at owner lookup.** The synthetic file owner lacks class information. In the class-member control, Rust resolves the source `bar`, the unary context function’s `apply` and the literal argument `4`. |
| `tests/run/curried-using-threading.scala` · `narrow()` | The call and its ordinary default getter receive the same contextual argument. | **Blocked in the namer.** Anonymous `given Context("alpha")` produces `UnsupportedGivenNameShape(New)`. A class-member control reaches a second obstacle. Rust requires an ordinary argument where Scala calls the getter. |
| `tests/pos/i8491.scala` · `Test.qux2` | The synthesized witness precedes `B` in the implicit clause and the getter is `qux2$default$2`. | **Outside the documented v1 subset.** Rust completes the two-parameter signature but rejects the definition with `UsingApplicationDeferred` at `B`. The combination of a synthesized context bound and a default was already listed as unsupported. |

The existing contextual argument and consumer fixtures provide additional
controls for structural givens, dependent argument paths, contextual defaults,
nested context functions, conversions, extensions, extractors, and binary
providers. They exercise different samples from the eleven sites above.
The [typer compatibility contract](typer-v0.1-compatibility.md#contextual-arguments-in-scala-39)
describes their coverage and the separate output contracts for JVM and TASTy.

## Work required for closure

1. **Recognize unproductive growth in by-name search.** Reject the increasing
   `Foo[T]` obligation as missing evidence at the call. Preserve the successful
   shared dictionary in the companion example and keep the resource limit as a
   safeguard. The [reference divergence check](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Implicits.scala#L1889-L1905) compares type size and covering sets after checking the by-name back-reference.
2. **Resolve the synthetic file owner.** Retest both top-level examples through
   normal method typing. One must identify the two ambiguous declarations and
   the other must apply the context function to `4`.
3. **Type member selection on the result of `summon`.** Retest the full `.show`
   expression while retaining the nested-scope evidence check.
4. **Name the anonymous constructor-form given, then retest the curried call.**
   Once the namer admits the file, verify that both the call and its ordinary
   default getter reuse the contextual argument. The reduced case makes the
   ordinary-default dependency independently observable.

Close v1 when the original in-scope sites meet their Scala observations and
the existing focused regressions remain green. Keep the context-bound/default
combination tracked separately under the existing contract. This decision
covers source typing. Broader language conformance, JVM emission, and TASTy
interoperability have their own acceptance criteria.

## Reproduction

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
