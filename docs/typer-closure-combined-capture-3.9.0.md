# Combined lexical closure capture: same-owner method and parameter

This note records the bounded combined acceptance root from issue #1888 for
Scala 3.9.0. The semantic change is in the existing source Typer capture path:
an unqualified reference to a same-owner instance method inside an ordinary
`Function1` lambda records the enclosing instance receiver. The reference still
resolves to the canonical method symbol.

## Reproduction

The exact current-`main` baseline inspected was
`616bfcacd2a42a43bf8c8ae3bdbe4a9595229e45`. The positive source is
[`CombinedCaptures.scala`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/CombinedCaptures.scala)
and its normalized snapshot was captured from Scala's actual `-Vprint:typer`
output at revision `777528f19a58e794c9954a42f433373472ec57f8`.

At that baseline, `Calculator.makeAdder` already typed as a complete source
method. Its first acceptance failure was the closure receipt: `Closure.env`
contained only the canonical `makeAdder.delta` identifier and omitted
`Calculator.this`, despite `inc` resolving as the class method. After the
change, the whole `DefDef` retains its declared `Function1[Int, Int]` result
and maps the source method, lambda, lambda parameter, lambda body, `inc` use,
and `delta` use through `SourceTypedIndex`. The synthetic lambda method is
owned by `makeAdder`; its `x` parameter belongs to that synthetic method.
`Closure.env` is exactly `[Calculator.this, makeAdder.delta]`, in first-use
order, and `inc` remains the canonical `Calculator.inc` symbol.

Focused Rust validation:

```sh
cargo test -p dotty-typer combined_enclosing_this_and_method_parameter_capture_types_whole_method
cargo test -p dotty-typer combined_capture_rejects_wrong_member_argument_and_function_result
```

Pinned Scala oracle validation (Temurin 21.0.8+9, eight SHA-verified artifacts):

```sh
SCALA39_ROOT=/tmp/scala3-3.9.0 \
JAVA_HOME=/home/vdrake6/.jdks/jdk-21 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/tmp/dotty-coursier-cache/https/repo1.maven.org/maven2 \
DOTTY_LEXICAL_CLOSURE_TMP_ROOT=/path/to/task-local/target/lexical-closure-contract-oracle \
crates/dotty-typer/tests/fixtures/lexical-closure-contract/run
```

The oracle accepts and snapshots the exact positive root. Its two adjacent
negative sources are rejected by Scala 3.9.0: `inc(true)` reports
`Required: Int`, and a declared `Int => Boolean` result reports
`Required: Boolean`. The Rust regression observes the corresponding
`ApplicationArgumentTypeMismatch` and expression-result conformance errors;
failed method typing leaves no typed method, RHS, or lambda identity mappings.

## Boundary

The receiver capture is recorded only for a same-owner class or trait method in
an ordinary single-parameter function literal, using the existing canonical
`ThisType` capture entry. This increment does not extend inherited member
resolution, generic inference, `FunctionN`, type relations, mutable captures,
or backend and TASTy output. Existing no-capture, parameter-only, explicit
instance-only, and shadowing tests remain the surrounding regression controls.
