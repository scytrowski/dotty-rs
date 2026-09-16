package dotty.baseline

import java.io.BufferedInputStream
import java.io.File
import java.net.URLClassLoader
import java.nio.file.{Files, Path, Paths, StandardCopyOption}
import java.util.jar.JarFile

import scala.collection.mutable.ArrayBuffer
import scala.jdk.CollectionConverters.*
import scala.quoted.*
import scala.tasty.inspector.*

object Main:
  private final case class Input(path: String, tastyPath: Path)
  private final case class Selection(paths: Set[String])
  private final case class Projection(
      declarations: List[(String, String, Option[Int])],
      shapes: Map[String, Int],
      parameterClauseCounts: Map[Int, Int]
  )

  def main(args: Array[String]): Unit =
    if args.length < 2 then
      throw IllegalArgumentException(
        "usage: tasty-baseline <output.json> [--classpath=<path[:path...]>] <file.tasty|directory|file.jar> ..."
      )

    val output = Paths.get(args.head)
    val classpath = args.tail
      .filter(_.startsWith("--classpath="))
      .flatMap(_.stripPrefix("--classpath=").split(File.pathSeparatorChar).toList)
      .map(Paths.get(_))
    val selectionArguments = args.tail.filter(_.startsWith("--select="))
    if selectionArguments.size > 1 then
      throw IllegalArgumentException("only one --select option is supported")
    val selection = selectionArguments.headOption.map { argument =>
      Selection(readSelection(Paths.get(argument.stripPrefix("--select="))))
    }
    val unsupportedOptions = args.tail.filter(_.startsWith("--")).filterNot(argument =>
      argument.startsWith("--classpath=") || argument.startsWith("--select=")
    )
    if unsupportedOptions.nonEmpty then
      throw IllegalArgumentException(s"unsupported option: ${unsupportedOptions.head}")

    val inputArguments = args.tail.filterNot(_.startsWith("--"))
    val materializedInputs = materialize(inputArguments.toList)
    val inputs = materializedInputs.filter(input => selection.forall(_.paths.contains(input.path)))
    selection.foreach { selected =>
      val found = inputs.iterator.map(_.path).toSet
      val missing = selected.paths.diff(found).toList.sorted
      if missing.nonEmpty then
        throw IllegalArgumentException(
          s"selection names missing .tasty inputs: ${missing.mkString(", ")}"
        )
    }
    if inputs.isEmpty then throw IllegalArgumentException("no .tasty files found")

    val jarUrls = inputArguments
      .filter(_.endsWith(".jar"))
      .map(argument => Paths.get(argument).toUri.toURL)
    val classpathUrls = classpath.map(_.toUri.toURL)
    val loaderUrls = (classpathUrls ++ jarUrls).toArray
    val contextLoader = Thread.currentThread().getContextClassLoader
    if loaderUrls.nonEmpty then
      Thread.currentThread().setContextClassLoader(URLClassLoader(loaderUrls, contextLoader))

    val fileNames = inputs.map(_.tastyPath.toString)
    val result = ArrayBuffer.empty[String]
    TastyInspector.inspectTastyFiles(fileNames)(new Inspector:
      def inspect(using quotes: Quotes)(tastys: List[Tasty[quotes.type]]): Unit =
        if tastys.size != inputs.size then
          throw IllegalStateException(
            s"Scala inspector returned ${tastys.size} trees for ${inputs.size} input files"
          )

        for (input, tasty) <- inputs.zip(tastys) do
          result += fileJson(input.path)(using quotes)(tasty.ast)
    )

    if result.size != inputs.size then
      throw IllegalStateException(
        s"Scala inspector produced ${result.size} projections for ${inputs.size} input files"
      )
    Files.writeString(
      output,
      s"{\"schema_version\":3,\"scala_version\":\"3.9.0\",\"files\":[${result.mkString(",")}]}\n"
    )

  private def materialize(arguments: List[String]): List[Input] =
    val temporaryRoots = ArrayBuffer.empty[Path]
    val inputs = arguments.flatMap { argument =>
      val path = Paths.get(argument)
      if Files.isDirectory(path) then
        val stream = Files.walk(path)
        try
          stream.iterator().asScala
            .filter(file => Files.isRegularFile(file) && file.toString.endsWith(".tasty"))
            .toList
            .sortBy(_.toString)
            .map(file =>
              Input(path.relativize(file).toString.replace(File.separatorChar, '/'), file)
            )
        finally stream.close()
      else if argument.endsWith(".jar") then
        val temporaryRoot = Files.createTempDirectory("dotty-tasty-baseline-")
        temporaryRoots += temporaryRoot
        extractJar(path, temporaryRoot)
      else if argument.endsWith(".tasty") then
        List(Input(path.getFileName.toString, path))
      else throw IllegalArgumentException(s"unsupported baseline input: $argument")
    }
    inputs

  private def readSelection(path: Path): Set[String] =
    if !Files.isRegularFile(path) then
      throw IllegalArgumentException(s"selection file does not exist: $path")
    val lines = Files.readAllLines(path).asScala
      .map(_.trim)
      .filter(line => line.nonEmpty && !line.startsWith("#"))
      .map(_.replace('\\', '/'))
      .toList
    val duplicates = lines.groupBy(identity).collect {
      case (line, occurrences) if occurrences.size > 1 => line
    }
    if duplicates.nonEmpty then
      throw IllegalArgumentException(
        s"selection file contains duplicate paths: ${duplicates.toList.sorted.mkString(", ")}"
      )
    lines.toSet

  private def extractJar(jarPath: Path, output: Path): List[Input] =
    val jar = JarFile(jarPath.toFile)
    try
      jar.entries().asScala
        .filter(entry => !entry.isDirectory && entry.getName.endsWith(".tasty"))
        .toList
        .sortBy(_.getName)
        .map { entry =>
          val target = output.resolve(entry.getName).normalize()
          if !target.startsWith(output) then
            throw IllegalArgumentException(
              s"JAR entry escapes extraction directory: ${entry.getName}"
            )
          Option(target.getParent).foreach(Files.createDirectories(_))
          val input = BufferedInputStream(jar.getInputStream(entry))
          try Files.copy(input, target, StandardCopyOption.REPLACE_EXISTING)
          finally input.close()
          Input(entry.getName, target)
        }
    finally jar.close()

  private def fileJson(path: String)(using quotes: Quotes)(tree: quotes.reflect.Tree): String =
    val projection = semanticProjection(tree)
    val declarations = projection.declarations.map { case (kind, name, clauses) =>
      val clauseJson = clauses.map(value => s",\"parameter_clauses\":$value").getOrElse("")
      s"{\"kind\":${json(kind)},\"name\":${json(name)}$clauseJson}"
    }
    val shapes = stringIntMapJson(projection.shapes)
    val parameterClauses = stringIntMapJson(
      projection.parameterClauseCounts.map { case (count, occurrences) =>
        count.toString -> occurrences
      }
    )
    s"{\"path\":${json(path)},\"declarations\":[${declarations.mkString(",")}],\"shapes\":$shapes,\"parameter_clause_counts\":$parameterClauses}"

  private def semanticProjection(using quotes: Quotes)(tree: quotes.reflect.Tree): Projection =
    import quotes.reflect.*
    val declarations = ArrayBuffer.empty[(String, String, Option[Int])]
    val shapes = scala.collection.mutable.Map.empty[String, Int]
    val parameterClauseCounts = scala.collection.mutable.Map.empty[Int, Int]
    val shapeKinds = Set(
      "Apply",
      "Block",
      "If",
      "Lambda",
      "Match",
      "New",
      "Return",
      "Try",
      "TypeApply",
      "Typed",
      "WhileDo"
    )
    class Collector extends TreeTraverser:
      override def traverseTree(current: Tree)(owner: Symbol): Unit =
        val symbol = current.symbol
        val kind = current.getClass.getSimpleName.stripSuffix("$")
        if shapeKinds.contains(kind) then
          shapes(kind) = shapes.getOrElse(kind, 0) + 1
        if Set("TypeDef", "DefDef", "ValDef").contains(kind) && symbol.exists then
          val name = symbol.fullName.split('.').lastOption.getOrElse("")
          val normalized = name.stripSuffix("$")
          if normalized.nonEmpty && !normalized.startsWith("_") && !normalized.startsWith("<") &&
              normalized.forall(char => char.isLetterOrDigit || char == '_') then
            val clauses = if kind == "DefDef" then Some(symbol.paramSymss.length) else None
            declarations += ((kind, normalized, clauses))
            clauses.foreach { count =>
              parameterClauseCounts(count) = parameterClauseCounts.getOrElse(count, 0) + 1
            }
        super.traverseTree(current)(owner)

    new Collector().traverseTree(tree)(Symbol.noSymbol)
    Projection(
      declarations.distinct.sortBy { case (kind, name, clauses) => (kind, name, clauses) }.toList,
      shapes.toMap,
      parameterClauseCounts.toMap
    )

  private def stringIntMapJson(values: Map[String, Int]): String =
    val entries = values.toList.sortBy(_._1).map { case (key, value) =>
      s"${json(key)}:$value"
    }
    s"{${entries.mkString(",")}}"

  private def json(value: String): String =
    val escaped = value.flatMap {
      case '"'  => "\\\""
      case '\\' => "\\\\"
      case '\b' => "\\b"
      case '\f' => "\\f"
      case '\n' => "\\n"
      case '\r' => "\\r"
      case '\t' => "\\t"
      case char if char < ' ' => f"\\u${char.toInt}%04x"
      case char => char.toString
    }
    s"\"$escaped\""
