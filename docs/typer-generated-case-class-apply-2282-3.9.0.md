# Generated case-class `apply` inference (#2282)

The oracle fixture
[`RenamedFactories.scala`](../crates/dotty-typer/tests/fixtures/generated-case-class-apply-2282/RenamedFactories.scala)
was compiled with Scala revision
`777528f19a58e794c9954a42f433373472ec57f8` (Scala 3.9.0), JDK 21, and the
checksum-pinned artifacts listed in
[`scala-artifacts.sha256.tsv`](../tools/typer-classpath-corpus-audit/either-companion-oracle/scala-artifacts.sha256.tsv).
The compiler's typer output contains:

```scala
issue2282oracle.Rightish.apply[Nothing, A](value)
issue2282oracle.Leftish.apply[B, Nothing](value)
```

The generated signatures are
`Rightish.apply[A, B](value: B): Rightish[A, B]` and
`Leftish.apply[A, B](value: A): Leftish[A, B]`. The first call infers the
field-bearing second parameter from the argument and minimizes only the
unused first parameter to canonical `Nothing`; the second does the reverse.
The enclosing method's `A` or `B` remains in its original argument position.
These are compiler outputs for the exact checked-in source fixture; the
parallel Rust regressions assert source class identity, ordered applied
arguments, outer parameter identity, both field positions, and conservative
rejection controls.

The same pinned compiler's output for the unchanged Scala library
`Either.swap` is recorded in the prior oracle artifact
`util-Either-oracle.txt`: its two calls are
`Right.apply[Nothing, A](a)` and `Left.apply[B, Nothing](b)`. The actual
Either source remains a separate whole-method acceptance gate; these
application results do not claim that its complete `swap` body types in
dotty-rs.
