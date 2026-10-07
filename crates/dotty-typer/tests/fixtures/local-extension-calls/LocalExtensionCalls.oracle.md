# Local extension call reference

`LocalExtensionCalls.scala` is the focused Scala 3.9.0 source fixture for a
single local extension with one explicit receiver and one plain argument
clause. The pinned reference revision is
`777528f19a58e794c9954a42f433373472ec57f8`.

The source call has the shape `Apply(Select(this, choose), [2])`. In the pinned
Scala implementation, `Typer.tryExtensionOrConversion` finds the extension
method and calls `extMethodApply` with the typed qualifier. `Applications.extMethodApply`
builds an `Apply` that supplies the receiver, and the source argument clause is
applied to that result. `tryApplyingExtensionMethod` also shows how Scala
replaces the synthesized callee with the original method reference. These
upstream implementation references establish the receiver-first tree shape:

- [`Typer.scala` at the pinned revision](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Typer.scala#L4307-L4318)
- [`Applications.scala`, `extMethodApply` and `tryApplyingExtensionMethod`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/compiler/src/dotty/tools/dotc/typer/Applications.scala#L2975-L3050)

That shape is:

```text
Apply(Apply(Ident(choose), [this]), [2])
```

The source sets the receiver and method name spans; the focused typer
regression asserts that the typed receiver argument retains the source
receiver span. Ordinary member precedence, applicability fallback, and nested
local shadowing are covered by neighboring unit tests.
