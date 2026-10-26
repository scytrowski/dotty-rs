# Exploratory Scala standard-library method audit

This is a separate exploratory sample for #1566. It does not replace or change
the frozen `package-term` cohort, its baseline, or its 369 local-method rows.

The sample uses Scala 3.9.0 at revision
`777528f19a58e794c9954a42f433373472ec57f8` and the source files listed in
[`typer-stdlib-root-method-sample-3.9.0.tsv`](typer-stdlib-root-method-sample-3.9.0.tsv).
It selects non-local method definitions with bodies and records their body
typing outcomes separately from the existing local-method results.

## Results

The audit parsed and named all three files. It attempted 8 member methods;
3 method bodies typed successfully:

| Method | Body typing result |
| --- | --- |
| `scala.Proxy.toString` | typed |
| `scala.ScalaReflectionException$.toString` | typed |
| `scala.util.LowPriorityNotGiven.default` | typed |
| `scala.Proxy.hashCode` | `UnstableSelectionPrefix` |
| `scala.Proxy.equals` | `MemberLookup` |
| `scala.util.NotGiven$.value` | `MissingDeclaredType` |
| `scala.util.NotGiven$.amb1` | `TermNameNotFound` |
| `scala.util.NotGiven$.amb2` | `TermNameNotFound` |

“Typed” here means the method body expression was accepted by
`type_expression` in the declaration's lexical context. It does not assert
that the entire method definition was emitted as typed AST or that the body
was checked against its declared result type. This is a small exploratory
signal, not a Scala compatibility or full-library coverage claim.

The two especially small candidates are the constant-string
`ScalaReflectionException$.toString` and `Proxy.toString`; the latter also
successfully exercises string concatenation with `self`. `NotGiven.default`
successfully resolves another source method through a generic method body.

Two normalized runs produced byte-identical focused reports. Both had zero
parser and namer failures. The machine output is retained locally at
`target/issue-1566-stdlib-method-audit.md` for this run; use the command below
to reproduce it.

```sh
SCALA39_ROOT=/path/to/pinned/scala JAVA_HOME=/path/to/jdk-21 \
SCALA39_JDK_RELEASE=21 \
tools/typer-classpath-corpus-audit/run \
  --focused-manifest docs/typer-stdlib-root-method-sample-3.9.0.tsv \
  --include-root-method-outcomes --verify-determinism \
  target/issue-1566-stdlib-method-audit.md
```
