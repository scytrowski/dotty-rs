import scala.jdk.CollectionConverters.*

object InheritedObjectWildcard:
  def convert(iterator: java.util.Iterator[String]): Iterator[String] =
    iterator.asScala
