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

    println("token\tstart\tend\tline_start\tname\tstring_value\tbase")
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
          base.toString
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
