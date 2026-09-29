object LocalStatementBodies:
  def forBody(values: List[Int]): Unit =
    for (value <- values)
      val adjusted = value + 1
      def result = adjusted
      consume(result)

  def branchBody(condition: Boolean): Int =
    if condition then
      val value = 1
      def result = value + 1
      result
    else 0
