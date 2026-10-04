# Scala 3.9.0 repeated parameter oracle

Compiler reference: Scala 3.9.0, revision
`777528f19a58e794c9954a42f433373472ec57f8`.

`RepeatedParameters.scala` is compiled with the pinned compiler and
`-Vprint:typer`. The checked-in `RepeatedParameters.scala39-typed-tree.txt`
records the relevant typed tree:

```scala
def strings(xs: Seq[String] @Repeated): Unit = ()
def boxes(xs: Seq[Box[Int]] @Repeated): Unit = ()
```

Scala 3.9 uses an annotated `Seq[T]` in its typed tree. This repository keeps
the source declaration marker as `Type::Repeated { element: T }`; its method
signature stores `T` with `MethodParam.varargs = true`. The semantic parity
test normalizes the pinned TASTy representation to that existing contract. It
does not claim that a repeated parameter reference in a method body has a
modeled `Seq[T]` value type.
