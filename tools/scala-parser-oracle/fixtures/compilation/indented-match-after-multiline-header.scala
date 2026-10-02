trait IndentedMatchAfterMultilineHeader {
  def f(prefix: Int,
      values: List[Int]): Int = values match
    case head :: tail => f(prefix + head, tail)
    case Nil => prefix
}
