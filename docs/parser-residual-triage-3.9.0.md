# Residual parser corpus cases triage (Scala 3.9.0)

This note completes the diagnosis requested by issue #440 for the two
representative files in corpus report #436. The parser was tested at
`358a9560` (after the template/end-marker fix in PR #454); Scala behavior was
checked against the repository-pinned 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`.

## `TreeInfo.scala`: braced match boundary

The full `compiler/src/dotty/tools/dotc/ast/TreeInfo.scala` source is accepted
by the pinned Scala parser oracle. The Rust parser reports:

```text
ExpectedToken: expected `}` to close match cases
span: 11904..11904 (UTF-8 byte offsets)
```

The location is the line break before the second `case` in `unbind` at
`TreeInfo.scala:338-341`:

```scala
def unbind(x: Tree): Tree = unsplice(x) match {
  case Bind(_, y) => unbind(y)
  case y          => y
}
```

This is not a standalone braced-match failure. The following reduced fragment
reproduces the mismatch and is accepted by the Scala 3.9 parser oracle:

```scala
trait T {
  def catchesAllOf(cdef: CaseDef, threshold: Type): Boolean =
    isDefaultCase(cdef) ||
    cdef.guard.isEmpty && {
      unbind(cdef.pat) match {
        case Typed(Ident(WILDCARD), tpt) => threshold <:< tpt.typeOpt
        case _ => false
      }
    }

  private val languageSubCategories = Set(experimental, deprecated)

  def languageImport(path: Tree): Option[TermName] = path match
    case Select(p1, name: TermName) if languageSubCategories.contains(name) =>
      languageImport(p1) match
        case Some(EmptyTermName) => Some(name)
        case _ => None
    case p1: RefTree if p1.name == language =>
      p1.qualifier match
        case EmptyTree => Some(EmptyTermName)
        case p2: RefTree if p2.name == scala =>
          p2.qualifier match
            case EmptyTree => Some(EmptyTermName)
            case Ident(ROOTPKG) => Some(EmptyTermName)
            case _ => None
        case _ => None
    case _ => None

  def unbind(x: Tree): Tree = unsplice(x) match {
    case Bind(_, y) => unbind(y)
    case y => y
  }
}
```

The failure needs the interaction: a braced block containing a braced match,
an indentation-style match with nested matches, and a following braced match.
The likely owner is parser case-region/delimiter handling, not source validity
or scanner tokenization. Dotty's `Parsers.scala` accepts both match forms and
returns to the enclosing template after consuming the matching `}`. This
should become a focused parser implementation issue; it should not be grouped
with the lexer finding below.

## `BCodeSkelBuilder.scala`: interpolation scanner diagnostics

`compiler/src/dotty/tools/backend/jvm/BCodeSkelBuilder.scala` is accepted by
the pinned Scala parser oracle. The Rust contextual scanner reports two lexer
diagnostics (UTF-8 byte offsets in the full source):

| Span | Message | Source location |
| --- | --- | --- |
| `39018..39019` | `unclosed interpolated string literal` | line 901, the `{` in `${` |
| `39164..39166` | `unclosed string literal` | line 903, the closing quote/comma after the splice |

The reduced valid Scala input is:

```scala
object T {
  val value = em"$dd${
    if (debug) "x" else ""
  }"
}
```

Scala 3.9 parses this input successfully. The Rust scanner instead emits
`Error` tokens for the splice/string boundaries. `parser-smoke-dump` normally
stops at scanner diagnostics; when those tokens are passed through to the
parser, the parser reports separator/top-level errors at the same locations.
Those are recovery fallout, not an independent parser cause. The owning
component is the raw lexer/interpolation state machine, which needs to resume
the enclosing interpolated string after scanning a multiline `${ ... }`
expression containing ordinary string literals. A follow-up belongs in
`dotty-lexer`; it must remain separate from the `TreeInfo.scala` parser issue.

## Recommended follow-ups

1. Parser: reproduce and fix case/delimiter ownership for the reduced
   `TreeInfo.scala` sequence, retaining a following-member assertion.
2. Lexer: support nested ordinary string literals in a multiline interpolated
   expression splice; test both the reduced sample and `BCodeSkelBuilder.scala`.

No parser or lexer behavior was changed as part of this triage. Verification
used `tools/scala-parser-oracle/run --mode compilation` on both full Scala
sources and on reduced reproductions, and the Rust smoke-dump on the same
inputs. The parser oracle confirmed Scala 3.9 acceptance; the report above
preserves the current Rust diagnostics rather than normalizing them away.
