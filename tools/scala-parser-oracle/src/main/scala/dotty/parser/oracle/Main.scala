package dotty.parser.oracle

import java.nio.file.{Files, Paths}

import dotty.tools.dotc.core.Contexts.ContextBase
import dotty.tools.dotc.core.Flags.{Given, Trait}
import dotty.tools.dotc.parsing.Parsers
import dotty.tools.dotc.util.SourceFile

object Main:
  def main(args: Array[String]): Unit =
    val (mode, path) = args.toList match
      case path :: Nil => ("expr", path)
      case "--mode" :: "pattern" :: path :: Nil => ("pattern", path)
      case "--batch" :: manifest :: Nil =>
        runBatch(manifest)
        return
      case _ =>
        throw IllegalArgumentException(
          "usage: scala-parser-oracle [--mode pattern] <source-file> | --batch manifest"
        )

    println(parseAndRender(mode, path))

  private def runBatch(manifest: String): Unit =
    Files.readString(Paths.get(manifest)).linesIterator
      .filter(_.nonEmpty)
      .foreach: line =>
        val fields = line.split("\\t", -1)
        if fields.length != 2 then
          throw IllegalArgumentException(s"invalid oracle manifest entry: $line")
        println(parseAndRender(fields(0), fields(1)))

  private def parseAndRender(mode: String, path: String): String =
    val sourcePath = Paths.get(path)
    val source = Files.readString(sourcePath)
    val sourceFile = SourceFile.virtual(sourcePath.toString, source)
    val context = (new ContextBase).initialCtx
    val parser = new Parsers.Parser(sourceFile)(using context)
    val tree = if mode == "pattern" then parser.pattern() else parser.expr()

    render(tree, source, placeholderBase(tree, source))

  private def render(
      tree: dotty.tools.dotc.ast.Trees.Tree[?],
      source: String,
      placeholderBase: Int
  ): String =
    val fields = collection.mutable.ArrayBuffer.empty[String]
    val normalizedKind = tree match
      case tuple: dotty.tools.dotc.ast.untpd.Tuple if childTrees(tuple).isEmpty => "Literal"
      case _ =>
        tree.getClass.getSimpleName.stripSuffix("$") match
          case "WhileDo" => "While"
          case "AppliedTypeTree" => "TypeApply"
          case "WildcardFunction" => "Function"
          case name => name
    fields += field("kind", quote(normalizedKind))
    fields += field("span", span(tree))

    tree match
      case ident: dotty.tools.dotc.ast.Trees.Ident[?] =>
        fields += field(
          "name",
          quote(normalizePlaceholderName(ident.name.toString, slice(ident, source), placeholderBase))
        )
        if isBackquotedIdent(ident, source) then
          fields += field("backquoted", "true")
      case select: dotty.tools.dotc.ast.Trees.Select[?] =>
        fields += field("name", quote(select.name.toString))
        if isBackquotedSelect(select, source) then
          fields += field("backquoted", "true")
      case named: dotty.tools.dotc.ast.Trees.NamedArg[?] =>
        fields += field("name", quote(named.name.toString))
      case apply: dotty.tools.dotc.ast.Trees.Apply[?] =>
        fields += field("apply_kind", quote(apply.applyKind.toString))
      case generator: dotty.tools.dotc.ast.untpd.GenFrom =>
        fields += field("check_mode", quote(generator.checkMode.toString))
      case bind: dotty.tools.dotc.ast.Trees.Bind[?] =>
        fields += field("name", quote(bind.name.toString))
      case tdef: dotty.tools.dotc.ast.Trees.TypeDef[?] =>
        val name = tdef.name.toString
        fields += field("name", quote(if isWildcardTypeParamSource(slice(tdef, source)) then "$type_wildcard" else name))
        if tdef.mods.is(Trait) then
          fields += field("trait", "true")
      case module: dotty.tools.dotc.ast.untpd.ModuleDef =>
        fields += field("name", quote(module.name.toString))
      case vdef: dotty.tools.dotc.ast.Trees.ValDef[?] =>
        if !slice(vdef, source).trim.startsWith("_") && !vdef.name.toString.startsWith("_$") then
          fields += field("name", quote(vdef.name.toString))
        if vdef.mods.is(Given) then
          fields += field("given", "true")
      case ddef: dotty.tools.dotc.ast.Trees.DefDef[?] =>
        fields += field("name", quote(ddef.name.toString))
        val clauses = ddef.paramss
        val typeParamCount = clauses.headOption.toList.flatMap(_.collect {
          case _: dotty.tools.dotc.ast.Trees.TypeDef[?] => 1
        }).size
        fields += field("type_param_count", typeParamCount.toString)
        fields += field(
          "param_clause_sizes",
          clauses.map(clause => clause.size).mkString("[", ",", "]")
        )
        fields += field(
          "using_clauses",
          clauses.map: clause =>
            clause.headOption match
              case Some(value: dotty.tools.dotc.ast.Trees.ValDef[?]) => value.mods.is(Given).toString
              case _ => "false"
          .mkString("[", ",", "]")
        )
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
      .collect {
        case (child, index) if !operatorIndex.contains(index) => render(child, source, placeholderBase)
      }
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

  private def placeholderBase(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): Int =
    placeholderIndices(tree, source).minOption.getOrElse(1)

  private def placeholderIndices(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): List[Int] =
    val own = tree match
      case ident: dotty.tools.dotc.ast.Trees.Ident[?] if slice(ident, source) == "_" =>
        ident.name.toString match
          case name if name.startsWith("_$") => name.drop(2).toIntOption.toList
          case name if name.startsWith("$placeholder_") => name.drop("$placeholder_".length).toIntOption.toList
          case _ => Nil
      case _ => Nil
    own ++ childTrees(tree).flatMap(child => placeholderIndices(child, source))

  private def normalizePlaceholderName(name: String, sourceText: String, base: Int): String =
    if sourceText == "_" then
      name match
        case suffix if suffix.startsWith("_$") =>
          suffix.drop(2).toIntOption.map(index => s"$$placeholder_${index - base}").getOrElse(name)
        case suffix if suffix.startsWith("$placeholder_") =>
          suffix.drop("$placeholder_".length).toIntOption.map(index => s"$$placeholder_${index - base}").getOrElse(name)
        case _ => name
    else name

  private def isWildcardTypeParamSource(sourceText: String): Boolean =
    val text = sourceText.trim
    if !text.startsWith("_") then false
    else
      text.drop(1).headOption match
        case None | Some(',') | Some(']') => true
        case Some(character) if character.isWhitespace => true
        case Some('<' | '>') => text.drop(2).startsWith(":")
        case _ => false

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
