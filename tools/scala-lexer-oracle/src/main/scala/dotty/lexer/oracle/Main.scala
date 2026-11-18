package dotty.lexer.oracle

import java.nio.file.{Files, Paths}

import dotty.tools.dotc.core.Contexts.ContextBase
import dotty.tools.dotc.parsing.{Scanners, Tokens}
import dotty.tools.dotc.util.SourceFile

object Main:
  def main(args: Array[String]): Unit =
    if args.length != 1 then
      throw IllegalArgumentException("usage: scala-lexer-oracle <source-file>")

    val path = Paths.get(args(0))
    val source = Files.readString(path)
    val sourceFile = SourceFile.virtual(path.toString, source)
    val context = (new ContextBase).initialCtx
    val scanner = Scanners.Scanner(sourceFile)(using context)

    println("token\tstart\tend\tline_start\tname\tstring_value\tbase\tlexical_kind")
    var afterInterpolator = false
    var done = false
    while !done do
      val token = Tokens.tokenString(scanner.token)
      val start = scanner.offset
      val lineStart = scanner.lineOffset
      val name = token match
        case "identifier" | "backquoted identifier" | "string interpolator" =>
          Option(scanner.name).fold("")(_.toString)
        case _ => ""
      val stringValue = token match
        case "character literal" | "integer literal" | "decimal literal" |
            "exponent literal" | "long literal" | "float literal" |
            "double literal" | "string literal" | "string part" |
            "interpolation id" => Option(scanner.strVal).getOrElse("")
        case _ => ""
      // Several distinct compiler token IDs share a display name. Record the
      // scanner's decision while the interpolation region is still current.
      val lexicalKind = scanner.token match
        case Tokens.STRINGPART => "string part"
        case Tokens.STRINGLIT if scanner.errOffset < start &&
            (afterInterpolator || scanner.currentRegion.isInstanceOf[Scanners.InString]) =>
          "string part"
        case Tokens.BACKQUOTED_IDENT => "backquoted identifier"
        case Tokens.IDENTIFIER if name.nonEmpty =>
          val first = name.codePointAt(0)
          if first == '_' || first == '$' || Character.isUnicodeIdentifierStart(first)
          then "identifier" else "operator"
        case Tokens.DECILIT => "decimal literal"
        case Tokens.EXPOLIT => "exponent literal"
        case _ => ""
      afterInterpolator = scanner.token == Tokens.INTERPOLATIONID
      val base = scanner.base
      scanner.nextToken()
      val end = scanner.offset
      println(
        List(
          token,
          start.toString,
          end.toString,
          lineStart.toString,
          escape(name),
          escape(stringValue),
          base.toString,
          lexicalKind
        ).mkString("\t")
      )
      done = token == Tokens.tokenString(Tokens.EOF)

  private def escape(value: String): String =
    value.flatMap:
      case '\\' => "\\\\"
      case '\t'  => "\\t"
      case '\n'  => "\\n"
      case '\r'  => "\\r"
      case char   => char.toString
