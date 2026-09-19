# Scala parser oracle

This tool exposes a small, stable JSON view of the syntax tree produced by
the Scala 3.9.0 compiler parser. It is a development oracle for `dotty-parser`,
not a production dependency of the Rust workspace.

The tool is intentionally pinned to Scala 3.9.0, JDK 25, and sbt 2.0.9. Run
it with the selected SDKMAN defaults:

```text
./run path/to/input.scala
```

With no argument, `run` reads the source from standard input. The output keeps
only parser-facing information: node kind, source span, names, literal source
text, and child nodes. It does not use `Tree.show`, because that output is a
compiler presentation rather than a compatibility protocol.

Compiler source offsets are UTF-16 code-unit offsets. Consumers comparing them
with Rust source spans must convert them to UTF-8 byte offsets first.

The fixtures cover the expression forms currently represented by the Rust
parser: identifiers, numeric and string literals, `this`, parentheses, the
empty tuple, tuples, simple selections and applications, `super`, `new`,
simple type applications, repeated suffix chains, and brace blocks.
Numeric suffix fixtures also cover `Long`, `Float`, and `Double` literals.
Backquoted identifier fixtures cover standalone and selected names. The
corpus intentionally does not claim coverage for the rest of Scala's
expression, type, or argument grammar.
