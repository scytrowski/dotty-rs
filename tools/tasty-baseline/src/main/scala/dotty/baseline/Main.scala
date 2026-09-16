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
    val inputArguments = args.tail.filterNot(_.startsWith("--classpath="))
    val inputs = materialize(inputArguments.toList)
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
      s"{\"schema_version\":1,\"scala_version\":\"3.9.0\",\"files\":[${result.mkString(",")}]}\n"
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
    s"{\"path\":${json(path)},\"nodes\":${fileTreeJson(tree)}}"

  private def fileTreeJson(using quotes: Quotes)(tree: quotes.reflect.Tree): String =
    import quotes.reflect.*
    val nodes = ArrayBuffer.empty[String]
    class Collector extends TreeTraverser:
      override def traverseTree(current: Tree)(owner: Symbol): Unit =
        val symbol = current.symbol
        val symbolName = if symbol.exists then symbol.fullName else ""
        val kind = current.getClass.getSimpleName.stripSuffix("$")
        nodes += s"{\"kind\":${json(kind)},\"symbol\":${json(symbolName)}}"
        super.traverseTree(current)(owner)

    new Collector().traverseTree(tree)(Symbol.noSymbol)
    s"[${nodes.mkString(",")}]"

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
