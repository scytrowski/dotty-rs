case class Setting[T](value: T)(using ordering: Ordering[T]):
  val rendered = value.toString
  val compared = ordering.compare(value, value)
