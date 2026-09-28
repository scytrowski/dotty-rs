package dotty.parser.oracle

import java.nio.file.{Files, Paths}

import dotty.tools.dotc.CompilationUnit
import dotty.tools.dotc.core.Contexts.ContextBase
import dotty.tools.dotc.core.Flags.{Abstract, Case, Enum, EnumCase, Final, Given, Implicit, Inline, Infix, Lazy, Mutable, Opaque, Open, Override, Param, ParamAccessor, Private, PrivateLocal, Protected, Sealed, Trait, Transparent}
import dotty.tools.dotc.parsing.Parsers
import dotty.tools.dotc.core.Flags.Package as PackageFlag
import dotty.tools.dotc.util.SourceFile
import scala.util.control.NonFatal

object Main:
  def main(args: Array[String]): Unit =
    val (mode, path) = args.toList match
      case path :: Nil => ("expr", path)
      case "--mode" :: "pattern" :: path :: Nil => ("pattern", path)
      case "--mode" :: "block" :: path :: Nil => ("expr", path)
      case "--mode" :: "block-erased" :: path :: Nil => ("expr", path)
      case "--mode" :: "compilation" :: path :: Nil => ("compilation", path)
      case "--batch" :: manifest :: Nil =>
        runBatch(manifest)
        return
      case _ =>
        throw IllegalArgumentException(
          "usage: scala-parser-oracle [--mode pattern|block|block-erased|compilation] <source-file> | --batch manifest"
        )

    println(parseAndRender(mode, path))

  private def runBatch(manifest: String): Unit =
    Files.readString(Paths.get(manifest)).linesIterator
      .filter(_.nonEmpty)
      .foreach: line =>
        val fields = line.split("\\t", -1)
        if fields.length != 2 then
          throw IllegalArgumentException(s"invalid oracle manifest entry: $line")
        try println(parseAndRender(fields(0), fields(1)))
        catch case NonFatal(error) => println(renderOracleFailure(fields(1), error))

  private def renderOracleFailure(path: String, error: Throwable): String =
    val message = Option(error.getMessage).getOrElse(error.getClass.getSimpleName)
    s"""{"kind":"OracleFailure","path":${quote(path)},"error":${quote(message)}}"""

  private def parseAndRender(mode: String, path: String): String =
    val sourcePath = Paths.get(path)
    val source = Files.readString(sourcePath)
    val sourceFile = SourceFile.virtual(sourcePath.toString, source)
    val context = (new ContextBase).initialCtx
    val unit = CompilationUnit(sourceFile, mustExist = false)(using context)
    val unitContext = context.fresh.setCompilationUnit(unit)
    val parser = new Parsers.Parser(sourceFile)(using unitContext)
    val tree = mode match
      case "pattern" => parser.pattern()
      case "compilation" => parser.compilationUnit()
      case _ => parser.expr()

    if mode == "compilation" && tree.getClass.getSimpleName.stripSuffix("$") == "EmptyTree" then
      renderEmptyCompilation(source)
    else
      render(tree, source, placeholderBase(tree, source))

  private def renderEmptyCompilation(source: String): String =
    val end = source.length
    s"""{"kind":"PackageDef","span":{"start":$end,"end":$end},"children":[{"kind":"Ident","span":{"start":$end,"end":$end},"name":"<empty>","children":[]}]}"""

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
          case "ByNameTypeTree" => "ByNameTypeTree"
          case "WildcardFunction" => "Function"
          case name => name
    fields += field("kind", quote(normalizedKind))
    fields += field("span", span(tree, source))

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
      case function: dotty.tools.dotc.ast.untpd.FunctionWithMods =>
        fields += field(
          "erased_params",
          function.erasedParams.map(_.toString).mkString("[", ",", "]")
        )
      case imported: dotty.tools.dotc.ast.Trees.Import[?] =>
        fields += field("selectors", renderSelectors(imported.selectors, source))
      case exported: dotty.tools.dotc.ast.Trees.Export[?] =>
        fields += field("selectors", renderSelectors(exported.selectors, source))
      case apply: dotty.tools.dotc.ast.Trees.Apply[?] =>
        fields += field("apply_kind", quote(apply.applyKind.toString))
      case generator: dotty.tools.dotc.ast.untpd.GenFrom =>
        fields += field("check_mode", quote(generator.checkMode.toString))
      case bind: dotty.tools.dotc.ast.Trees.Bind[?] =>
        fields += field("name", quote(bind.name.toString))
      case tdef: dotty.tools.dotc.ast.Trees.TypeDef[?] =>
        val name = tdef.name.toString
        fields += field("name", quote(if isWildcardTypeParamSource(slice(tdef, source)) then "$type_wildcard" else name))
        slice(tdef, source).trim.headOption match
          case Some('+') => fields += field("variance", quote("covariant"))
          case Some('-') => fields += field("variance", quote("contravariant"))
          case _ =>
        if !isTypeDefinitionSource(slice(tdef, source)) && tdef.mods.is(Param) then
          fields += field("param", "true")
        if !isTypeDefinitionSource(slice(tdef, source)) && tdef.mods.isAllOf(PrivateLocal) then
          fields += field("private_local", "true")
        if tdef.mods.is(Trait) then
          fields += field("trait", "true")
        if tdef.mods.is(Enum) then
          fields += field("enum", "true")
        if isTypeDefinitionSource(slice(tdef, source)) || tdef.mods.flags.isAllOf(EnumCase) then
          fields ++= renderDefinitionMetadata(tdef.mods, tdef, source, placeholderBase, includeMutable = false)
      case bound: dotty.tools.dotc.ast.untpd.ContextBoundTypeTree =>
        fields += field("parameter", quote(bound.paramName.toString))
        if bound.ownName.toString.nonEmpty then
          fields += field("name", quote(bound.ownName.toString))
      case module: dotty.tools.dotc.ast.untpd.ModuleDef =>
        fields += field("name", quote(module.name.toString))
        if module.mods.is(PackageFlag) then
          fields += field("package_object", "true")
        fields ++= renderDefinitionMetadata(module.mods, module, source, placeholderBase, includeMutable = false)
      case vdef: dotty.tools.dotc.ast.Trees.ValDef[?] =>
        if !slice(vdef, source).trim.startsWith("_") && !vdef.name.toString.startsWith("_$") then
          fields += field("name", quote(vdef.name.toString))
        if vdef.mods.is(Given) then
          fields += field("given", "true")
        if vdef.mods.is(ParamAccessor) then
          fields += field("param_accessor", "true")
        if vdef.mods.isAllOf(PrivateLocal) then
          fields += field("private_local", "true")
        if vdef.mods.is(Mutable) && (vdef.mods.is(ParamAccessor) || vdef.mods.isAllOf(PrivateLocal)) then
          fields += field("mutable", "true")
        fields ++= renderDefinitionMetadata(vdef.mods, vdef, source, placeholderBase)
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
        fields += field(
          "implicit_clauses",
          clauses.map: clause =>
            clause.headOption match
              case Some(value: dotty.tools.dotc.ast.Trees.ValDef[?]) => value.mods.is(Implicit).toString
              case _ => "false"
          .mkString("[", ",", "]")
        )
        fields ++= renderDefinitionMetadata(ddef.mods, ddef, source, placeholderBase, includeMutable = false)
      case ext if normalizedKind == "ExtMethods" =>
        val clauses = ext.productElement(0).asInstanceOf[List[List[dotty.tools.dotc.ast.Trees.Tree[?]]]]
        fields += field("param_clause_sizes", clauses.map(_.size).mkString("[", ",", "]"))
        fields += field(
          "using_clauses",
          clauses.map: clause =>
            clause.headOption match
              case Some(value: dotty.tools.dotc.ast.Trees.ValDef[?]) => value.mods.is(Given).toString
              case _ => "false"
          .mkString("[", ",", "]")
        )
        fields += field(
          "implicit_clauses",
          clauses.map: clause =>
            clause.headOption match
              case Some(value: dotty.tools.dotc.ast.Trees.ValDef[?]) => value.mods.is(Implicit).toString
              case _ => "false"
          .mkString("[", ",", "]")
        )
      case patdef: dotty.tools.dotc.ast.untpd.PatDef =>
        fields ++= renderDefinitionMetadata(patdef.mods, patdef, source, placeholderBase)
      case literal: dotty.tools.dotc.ast.Trees.Literal[?] =>
        fields += field("literal", quote(slice(literal, source)))
      case number: dotty.tools.dotc.ast.untpd.Number =>
        fields += field("literal", quote(slice(number, source)))
      case tuple: dotty.tools.dotc.ast.untpd.Tuple if childTrees(tuple).isEmpty =>
        fields += field("literal", quote(slice(tuple, source)))
      case _ =>

    if normalizedKind == "InterpolatedString" then
      fields += field("prefix", quote(slice(tree, source).takeWhile(_ != '"')))

    tree match
      case postfix: dotty.tools.dotc.ast.untpd.PostfixOp =>
        fields += field("operator", quote(postfix.op.name.toString))
      case _ =>

    val rawChildren =
      if normalizedKind == "InterpolatedString" then
        childTrees(tree).flatMap: child =>
          if child.getClass.getSimpleName.stripSuffix("$") == "Thicket" then childTrees(child)
          else child :: Nil
      else tree match
        case application: dotty.tools.dotc.ast.Trees.Apply[?] =>
          childTrees(tree).filterNot: child =>
            application.args.lastOption.contains(child) && isTrailingCommaPlaceholder(child, source)
        case _ => childTrees(tree)
    val operatorIndex = normalizedKind match
      case "PrefixOp"  => Some(0)
      case "InfixOp"   => Some(1)
      case "PostfixOp" => Some(1)
      case _            => None
    if normalizedKind != "PostfixOp" then
      operatorIndex.flatMap(index => rawChildren.lift(index)).foreach: operatorTree =>
        fields += field("operator", quote(operatorName(operatorTree, source)))
    val children = rawChildren.zipWithIndex
      .collect {
        case (child, index) if !operatorIndex.contains(index) => render(child, source, placeholderBase)
      }
      .mkString("[", ",", "]")
    fields += field("children", children)
    fields.mkString("{", ",", "}")

  private def renderDefinitionMetadata(
      mods: dotty.tools.dotc.ast.untpd.Modifiers,
      tree: dotty.tools.dotc.ast.Trees.Tree[?],
      source: String,
      base: Int,
      includeMutable: Boolean = true
  ): List[String] =
    val enabled = List(
      "abstract" -> Abstract,
      "final" -> Final,
      "sealed" -> Sealed,
      "case" -> Case,
      "implicit" -> Implicit,
      "lazy" -> Lazy,
      "override" -> Override,
      "inline" -> Inline,
      "transparent" -> Transparent,
      "opaque" -> Opaque,
      "open" -> Open,
      "infix" -> Infix,
      "var" -> Mutable,
      "given" -> Given
    ).collect {
      case (name, flag)
          if (includeMutable || name != "var")
              && mods.is(flag)
              && (name != "case" || !mods.flags.isAllOf(EnumCase)) => name
    }.toSet
    val sourceText = slice(tree, source)
    val sourceWords = sourceText.split("[^A-Za-z]+").toSet
    val enabledWithSource =
      enabled ++ (if includeMutable && sourceWords.contains("var") then Set("var") else Set.empty)
    val keywordIndex = tree match
      case _: dotty.tools.dotc.ast.Trees.ValDef[?] if isValueDefinitionSource(sourceText) =>
        List(sourceText.indexOf('='), sourceText.indexOf(':'))
          .filter(_ >= 0)
          .minOption
          .getOrElse(sourceText.length)
      case ddef: dotty.tools.dotc.ast.Trees.DefDef[?] if ddef.name.toString == "<init>" =>
        val parameterClause = sourceText.indexOf('(')
        if parameterClause < 0 then sourceText.length else parameterClause
      case _: dotty.tools.dotc.ast.Trees.DefDef[?] =>
        indexOfWord(sourceText, "def").getOrElse(sourceText.length)
      case _: dotty.tools.dotc.ast.Trees.TypeDef[?] =>
        List("type", "class", "trait", "enum")
          .flatMap(indexOfWord(sourceText, _))
          .minOption
          .getOrElse(sourceText.length)
      case _: dotty.tools.dotc.ast.untpd.ModuleDef =>
        indexOfWord(sourceText, "object").getOrElse(sourceText.length)
      case _ => sourceText.length
    val ordered = sourceText
      .take(keywordIndex)
      .split("[^A-Za-z]+")
      .toList
      .filter(enabledWithSource.contains)
    val prefixText = sourceText.take(keywordIndex)
    val prefixWords = prefixText.split("[^A-Za-z]+").toSet
    val modifiers = ordered.map(name => quote(name)).mkString("[", ",", "]")
    val hasQualifiedVisibility = !mods.privateWithin.isEmpty
    val visibility =
      if mods.flags.isAllOf(EnumCase) then
        if mods.is(Private) || (hasQualifiedVisibility && prefixWords.contains("private")) then "private"
        else if mods.is(Protected) || (hasQualifiedVisibility && prefixWords.contains("protected")) then "protected"
        else ""
      else if prefixWords.contains("private") then "private"
      else if prefixWords.contains("protected") then "protected"
      else ""
    val sourceQualifier =
      "(?:private|protected)\\s*\\[([^]]+)\\]".r
        .findFirstMatchIn(prefixText)
        .map(_.group(1).trim)
        .getOrElse("")
    val qualifier =
      if visibility.isEmpty then "null"
      else if !mods.privateWithin.isEmpty then quote(mods.privateWithin.toString)
      else if prefixText.contains('[') && sourceQualifier.nonEmpty then quote(sourceQualifier)
      else "null"
    val annotationTrees = mods.annotations.map(annotation =>
      render(annotation, source, placeholderBase(annotation, source))
    ).mkString("[", ",", "]")
    val metadata = List(
      field("modifiers", modifiers),
      field("visibility", if visibility.isEmpty then "null" else quote(visibility)),
      field("visibility_qualifier", qualifier),
      field("annotations", annotationTrees)
    )
    if mods.flags.isAllOf(EnumCase) then metadata :+ field("enum_case", "true") else metadata

  private def indexOfWord(source: String, word: String): Option[Int] =
    val index = source.indexOf(word)
    if index >= 0 then Some(index) else None

  private def isValueDefinitionSource(source: String): Boolean =
    source.split("[^A-Za-z]+").exists(word => word == "val" || word == "var")

  private def isTypeDefinitionSource(source: String): Boolean =
    source.split("[^A-Za-z]+").exists(word =>
      word == "type" || word == "class" || word == "trait" || word == "object" || word == "enum"
    )

  private def childTrees(tree: dotty.tools.dotc.ast.Trees.Tree[?]): List[dotty.tools.dotc.ast.Trees.Tree[?]] =
    def collect(value: Any): List[dotty.tools.dotc.ast.Trees.Tree[?]] = value match
      case child: dotty.tools.dotc.ast.Trees.Tree[?] if child.span.exists => child :: Nil
      case _: dotty.tools.dotc.ast.Trees.Tree[?] => Nil
      case values: Iterable[?] => values.toList.flatMap(collect)
      case _ => Nil

    tree match
      case imported: dotty.tools.dotc.ast.Trees.Import[?] => imported.expr :: Nil
      case exported: dotty.tools.dotc.ast.Trees.Export[?] => exported.expr :: Nil
      case packageDef: dotty.tools.dotc.ast.Trees.PackageDef[?] =>
        packageDef.pid :: packageDef.stats
      case _ => tree.productIterator.toList.flatMap(collect)

  private def renderSelectors(
      selectors: List[dotty.tools.dotc.ast.untpd.ImportSelector],
      source: String
  ): String =
    selectors.map: selector =>
      val fields = collection.mutable.ArrayBuffer.empty[String]
      fields += field("name", quote(selector.name.toString))
      selector.renamed match
        case ident: dotty.tools.dotc.ast.Trees.Ident[?] if ident.span.exists =>
          fields += field("rename", quote(ident.name.toString))
        case tree if tree.span.exists =>
          fields += field("rename_tree", render(tree, source, placeholderBase(tree, source)))
        case _ =>
      selector.bound match
        case tree if tree.span.exists =>
          fields += field("bound", render(tree, source, placeholderBase(tree, source)))
        case _ =>
      s"{${fields.mkString(",")}}"
    .mkString("[", ",", "]")

  private def span(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): String =
    if tree.span.exists then
      val start =
        if tree.isInstanceOf[dotty.tools.dotc.ast.Trees.ValDef[?]] then
          val text = slice(tree, source)
          val leading = text.dropWhile(_.isWhitespace)
          if leading.startsWith("erased ") then
            tree.span.start + text.indexOf(leading) + leading.indexOf(' ') + 1 +
              leading.drop(7).indexWhere(!_.isWhitespace)
          else tree.span.start
        else tree.span.start
      s"{\"start\":$start,\"end\":${tree.span.end}}"
    else
      "null"

  private def slice(tree: dotty.tools.dotc.ast.Trees.Tree[?], source: String): String =
    if !tree.span.exists then return ""
    val start = math.max(0, math.min(tree.span.start, source.length))
    val end = math.max(start, math.min(tree.span.end, source.length))
    source.substring(start, end)

  // Dotty's generic commaSeparated helper parses one extra `???` tree when an
  // application ends in a trailing comma. Its span covers the trivia up to
  // `)`, or is zero-width when there is no trivia. Normalize only that final
  // synthetic child; explicit `???` arguments retain their source span.
  private def isTrailingCommaPlaceholder(
      tree: dotty.tools.dotc.ast.Trees.Tree[?],
      source: String
  ): Boolean =
    val isQuestionMarkSelect = tree match
      case select: dotty.tools.dotc.ast.Trees.Select[?] => select.name.toString == "???"
      case _ => false
    if !isQuestionMarkSelect || !tree.span.exists then return false
    val start = tree.span.start
    val end = tree.span.end
    if start <= 0 || start > end || end >= source.length ||
        source.charAt(start - 1) != ',' || source.charAt(end) != ')' then
      return false

    var cursor = start
    while cursor < end do
      if source.charAt(cursor).isWhitespace then cursor += 1
      else if source.startsWith("//", cursor) then
        cursor += 2
        while cursor < end && source.charAt(cursor) != '\n' do cursor += 1
      else if source.startsWith("/*", cursor) then
        cursor += 2
        var nesting = 1
        while cursor < end && nesting > 0 do
          if source.startsWith("/*", cursor) then
            nesting += 1
            cursor += 2
          else if source.startsWith("*/", cursor) then
            nesting -= 1
            cursor += 2
          else cursor += 1
        if nesting != 0 then return false
      else return false

    true

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
