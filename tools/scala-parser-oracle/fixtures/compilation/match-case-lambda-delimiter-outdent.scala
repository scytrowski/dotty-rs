object MatchCaseLambdaLayout {
  def collect(values: List[Int]) = values match {
    case items =>
      val mapped = items.map(item =>
        helper(Other(item)).toString)
      def result = mapped
      result
  }
}
