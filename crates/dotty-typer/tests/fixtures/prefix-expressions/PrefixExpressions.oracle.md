# Scala 3.9.0 prefix expression oracle

The source fixture was compiled with the pinned Scala 3.9.0 compiler and
`-Vprint:typer`. Each prefix expression becomes a selected `unary_` member.
The typed output contains a `Select` with no `Apply` for the parameterless
methods in `Box` and the stable value in `ValueBox`.

The additional probe `class EmptyParenBox { def unary_!(): EmptyParenBox = this };
def probe(value: EmptyParenBox): EmptyParenBox = !value` fails with E100:
`method unary_! in class EmptyParenBox must be called with () argument`.
The source fixture excludes that invalid form.
