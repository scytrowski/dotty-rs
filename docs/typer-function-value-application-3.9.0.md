# Function-value application, Scala 3.9.0

This document tracks issue #1706. The implementation scope is application of
an existing canonical Scala function value, with ordinary `Function1` and
`Function2` as the initial bounded cases. It does not cover implicit/SAM
conversion, contextual functions, `FunctionXXL`, or polymorphic function
values.

## Pinned baseline

The Scala source is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`; the compiler and library artifacts
are 3.9.0 and the local runtime is Temurin JDK 21.0.8+9. The dotty-rs baseline
is merge commit `88553158589e8336b3905b1cd03330b3600cc29e`.

The focused local-method audit selected only
`library/src/scala/collection/IterableOnce.scala`. It attempted the two local
`loop` definitions in `IterableOnceOps.foldl` and `IterableOnceOps.foldr`:

| Source identity | Enclosing method | Function value | First blocker |
| --- | --- | --- | --- |
| `IterableOnce.scala#tree=2043` | `foldl`, local `loop` at line 708 | `op: (B, X) => B` | `ApplicationCalleeNotMethod` at call tree 2051 |
| `IterableOnce.scala#tree=2097` | `foldr`, local `loop` at line 715 | `op: (X, B) => B` | `ApplicationCalleeNotMethod` at call tree 2111 |

The compact frozen rows, including the actual source lines, are in
[`typer-function-value-application-3.9.0.tsv`](typer-function-value-application-3.9.0.tsv).
The local audit ran twice serially and produced byte-identical reports with
SHA-256
`022b607195bd894769db0149c4000d9b1de2858d7f41d26fb61ce85a97302146`.
Both local methods remained untyped at baseline.

The resolver observations show a canonical `scala.Function2` TASTy class and
its `scala.annotation.retainsCap` dependency were available. The actual gap is
that application resolution leaves this external canonical function class
untouched: `resolve_apply_function` accepted module classes and source classes
or traits, but not the classpath-backed `scala.FunctionN` family. This is not
evidence of a missing `apply` member or a reason to invent one by name.

## Scala oracle

[`FunctionValueApplication.scala`](../crates/dotty-typer/tests/fixtures/function-value-application/FunctionValueApplication.scala)
contains extracted versions of both real `IterableOnce` roots plus minimal
`Function1`, `Function2`, generic, and nested application controls. Pinned
Scala 3.9.0 compiled it with `-Yexplicit-nulls -Ystop-after:typer -Vprint:typer`.
The oracle emitted `op.apply(acc, seq.apply(at))` and
`op.apply(seq.apply(at - 1), acc)` with the enclosing type parameters retained;
the direct controls likewise emitted `function.apply(...)`. No adaptation or
inferred `Any` is involved.

Reproduce the baseline local audit by setting `SCALA39_ROOT`, `JAVA_HOME`,
`SCALA39_JDK_RELEASE=21`, and `COURSIER_MAVEN_ROOT` to the pinned inputs, then:

```sh
tools/typer-classpath-corpus-audit/run \
  --focused-manifest target/issue-1706-artifacts/iterableonce.manifest \
  --verify-determinism target/issue-1706-artifacts/iterableonce-baseline.md
```

Compile the oracle fixture with the eight exact Scala 3.9.0 jars listed in
`tools/typer-classpath-corpus-audit/run` on `SCALA39_CLASSPATH`:

```sh
java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Yexplicit-nulls -Ystop-after:typer \
  -Vprint:typer -d /tmp/function-value-application-oracle \
  crates/dotty-typer/tests/fixtures/function-value-application/FunctionValueApplication.scala
```

## Implementation results

Pending. The before/after table will retain these same two source identities
and report blocker movement separately from full local-method typing.
