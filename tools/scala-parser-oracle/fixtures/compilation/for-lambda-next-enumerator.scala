object Enumerators:
  def values: List[Int] = List(1)

  def run: List[Int] =
    for
      value <- values
      transform = (item: Int) =>
        val incremented = item + 1
        import scala.Predef.*
        incremented
      result <- values.map(transform(value))
    yield result
