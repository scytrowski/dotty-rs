# Local extension call oracle

`LocalExtensionCalls.scala` is the focused Scala 3.9.0 source fixture for a
single local extension with one explicit receiver and one plain argument
clause. The pinned reference revision is
`777528f19a58e794c9954a42f433373472ec57f8`.

The source call has the shape `Apply(Select(this, choose), [2])`. Its typed
application must pass the receiver to the local method before applying the
ordinary argument clause:

```text
Apply(Apply(Ident(choose), [this]), [2])
```

The focused typer regression asserts this tree shape and checks that the typed
receiver argument keeps the source receiver span. Ordinary member precedence,
applicability fallback, and nested local shadowing are covered by neighboring
unit tests.
