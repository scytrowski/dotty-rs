# Scala parser oracle

This tool exposes a small, stable JSON view of the syntax tree produced by
the Scala 3.9.0 compiler parser. It is a development oracle for `dotty-parser`,
not a production dependency of the Rust workspace.

The tool is intentionally pinned to Scala 3.9.0, JDK 25, and sbt 2.0.9. Run
it with the selected SDKMAN defaults:

```text
./run path/to/input.scala
./run --mode pattern path/to/pattern.scala
./run --mode compilation path/to/source-unit.scala
./run --batch path/to/manifest.tsv
```

The batch manifest contains one tab-separated `mode` and absolute source path
per line. `compare.sh` uses this mode automatically: Scala parses the whole
corpus in one JVM, while the Rust dump tool is built and run once. This avoids
restarting sbt, Scala, Cargo, and Python for every fixture.

With no argument, `run` reads the source from standard input. The output keeps
only parser-facing information: node kind, source span, names, literal source
text, application kind, operators, and child nodes. It does not use `Tree.show`, because that output is a
compiler presentation rather than a compatibility protocol.

Compiler source offsets are UTF-16 code-unit offsets. Consumers comparing them
with Rust source spans must convert them to UTF-8 byte offsets first.

The default fixtures cover the expression forms currently represented by the Rust
parser: identifiers, numeric and string literals, `this`, parentheses, the
empty tuple, tuples, simple selections and applications, `super`, `new`,
simple type applications, repeated suffix chains, and brace blocks.
Numeric suffix fixtures also cover `Long`, `Float`, and `Double` literals.
Backquoted identifier fixtures cover standalone and selected names. The
corpus also covers the initial `Expr1` subset: ordinary assignment, the
narrow bare-identifier named-argument form, `using` argument lists, expression
type ascriptions, indented colon arguments, and `if`/`while` expressions,
including basic indented bodies, plus braced and indented `match` expressions
with case patterns, guards, and bodies. The current control-flow subset also
includes `throw`, bare/value `return`, and source-level `try`/`catch`/`finally`,
including their basic indented forms.
The lambda fixtures cover explicit function literals with single, empty,
multiple, typed, wildcard, and context-function parameters, plus nested,
applied, argument, block, and indented bodies. Context-function parameters are
compared through their normalized `given` marker on `ValDef`.
The polyfunction fixtures cover type-parameter clauses with lower and upper
bounds, wildcard type parameters, and value-parameter function bodies. The
placeholder fixtures cover synthetic-function lowering for infix expressions,
selections, applications, multiple parameters, and nested placeholder scopes;
the oracle normalizes Dotty's `WildcardFunction` and generated names to the
shared Rust representation.
It intentionally does not claim coverage for the rest of Scala's expression,
type, or argument grammar.

Fixtures under `fixtures/patterns/` use the explicit `pattern` mode. Scala
mode calls Dotty's real `Parser.pattern()` entry and Rust mode calls the
parser's standalone pattern-fragment entry; pattern fixtures are not wrapped
in synthetic `match` expressions. The normalized tree compares `Bind`,
`Alternative`, `Typed`, extractor-style source `Apply`, and named pattern
arguments as they appear before semantic extractor lowering.

Fixtures under `fixtures/definitions/` cover the initial class-like
definition subset in block mode: class, trait, and object nodes; primary
constructor clauses; simple parent applications; and braced or indented
template bodies. They also cover source annotations, hard and supported soft
modifiers, and qualified visibility. The normalized definition metadata
compares modifier order, visibility, visibility qualifiers, and annotation
trees; `ModuleDef.metadata` is included just like metadata on the other
definition nodes. The renderer exposes the source-level `Trait` distinction
and keeps constructor parent applications in the same `New`/`Select`/`Apply`
shape as the Scala parser.

Fixtures under `fixtures/compilation/` use `compilation` mode. Scala calls the
real `Parser.compilationUnit()` entry and Rust calls `parse_compilation_unit`.
The corpus covers empty and explicit package roots, nested packages, ordered
top-level definitions, and import/export clauses. Dotty returns `EmptyTree`
for an empty compilation unit; the Scala renderer normalizes that one case to
the Rust parser's documented zero-width empty `PackageDef` root.
