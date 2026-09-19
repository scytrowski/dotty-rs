package dotty.parser.oracle

import java.nio.file.{Files, Paths}

import dotty.tools.dotc.core.Contexts.ContextBase
import dotty.tools.dotc.parsing.Parsers
import dotty.tools.dotc.util.SourceFile

object Main:
  def main(args: Array[String]): Unit =
    val (mode, path) = args.toList match
      case path :: Nil => ("expr", path)
      case "--mode" :: "pattern" :: path :: Nil => ("pattern", path)
      case _ => throw IllegalArgumentException("usage: scala-parser-oracle [--mode pattern] <source-file>")

    val sourcePath = Paths.get(path)
    val source = Files.readString(sourcePath)
    val sourceFile = SourceFile.virtual(sourcePath.toString, source)
    val context = (new ContextBase).initialCtx
    val parser = new Parsers.Parser(sourceFile)(using context)
    val tree = if mode == "pattern" then parser.pattern() else parser.expr()

    println(render(tree, source))

  private def render(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): String =
    val fields = collection.mutable.ArrayBuffer.empty[String]
    val normalizedKind = tree match
      case tuple: dotty.tools.dotc.ast.untpd.Tuple if childTrees(tuple).isEmpty => "Literal"
      case _ =>
        tree.getClass.getSimpleName.stripSuffix("$") match
          case "WhileDo" => "While"
          case name => name
    fields += field("kind", quote(normalizedKind))
    fields += field("span", span(tree))

    tree match
      case ident: dotty.tools.dotc.ast.Trees.Ident[?] =>
        fields += field("name", quote(ident.name.toString))
        if isBackquotedIdent(ident, source) then
          fields += field("backquoted", "true")
      case select: dotty.tools.dotc.ast.Trees.Select[?] =>
        fields += field("name", quote(select.name.toString))
        if isBackquotedSelect(select, source) then
          fields += field("backquoted", "true")
      case named: dotty.tools.dotc.ast.Trees.NamedArg[?] =>
        fields += field("name", quote(named.name.toString))
      case bind: dotty.tools.dotc.ast.Trees.Bind[?] =>
        fields += field("name", quote(bind.name.toString))
      case literal: dotty.tools.dotc.ast.Trees.Literal[?] =>
        fields += field("literal", quote(slice(literal, source)))
      case number: dotty.tools.dotc.ast.untpd.Number =>
        fields += field("literal", quote(slice(number, source)))
      case tuple: dotty.tools.dotc.ast.untpd.Tuple if childTrees(tuple).isEmpty =>
        fields += field("literal", quote(slice(tuple, source)))
      case _ =>

    val rawChildren = childTrees(tree)
    val operatorIndex = normalizedKind match
      case "PrefixOp"  => Some(0)
      case "InfixOp"   => Some(1)
      case "PostfixOp" => Some(1)
      case _            => None
    operatorIndex.flatMap(index => rawChildren.lift(index)).foreach: operatorTree =>
      fields += field("operator", quote(operatorName(operatorTree, source)))
    val children = rawChildren.zipWithIndex
      .collect { case (child, index) if !operatorIndex.contains(index) => render(child, source) }
      .mkString("[", ",", "]")
    fields += field("children", children)
    fields.mkString("{", ",", "}")

  private def childTrees(tree: dotty.tools.dotc.ast.Trees.Tree[?]): List[dotty.tools.dotc.ast.Trees.Tree[?]] =
    def collect(value: Any): List[dotty.tools.dotc.ast.Trees.Tree[?]] = value match
      case child: dotty.tools.dotc.ast.Trees.Tree[?] if child.span.exists => child :: Nil
      case _: dotty.tools.dotc.ast.Trees.Tree[?] => Nil
      case values: Iterable[?] => values.toList.flatMap(collect)
      case _ => Nil

    tree.productIterator.toList.flatMap(collect)

  private def span(tree: dotty.tools.dotc.ast.Trees.Tree[?]): String =
    if tree.span.exists then
      s"{\"start\":${tree.span.start},\"end\":${tree.span.end}}"
    else
      "null"

  private def slice(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): String =
    if !tree.span.exists then return ""
    val start = math.max(0, math.min(tree.span.start, source.length))
    val end = math.max(start, math.min(tree.span.end, source.length))
    source.substring(start, end)

  private def operatorName(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): String =
    slice(tree, source) match
      case text if text.startsWith("`") && text.endsWith("`") => text.drop(1).dropRight(1)
      case text => text

  private def isBackquotedIdent(ident: dotty.tools.dotc.ast.Trees.Ident[?], source: String): Boolean =
    val text = slice(ident, source)
    text.startsWith("`") && text.endsWith("`")

  private def isBackquotedSelect(select: dotty.tools.dotc.ast.Trees.Select[?], source: String): Boolean =
    val text = slice(select, source)
    val dot = text.lastIndexOf('.')
    dot >= 0 && text.substring(dot + 1).startsWith("`")

  private def field(name: String, value: String): String =
    s"${quote(name)}:$value"

  private def quote(value: String): String =
    val escaped = value.flatMap:
      case '\\' => "\\\\"
      case '"'  => "\\\""
      case '\n' => "\\n"
      case '\r' => "\\r"
      case '\t' => "\\t"
      case char => char.toString
    s"\"$escaped\""
