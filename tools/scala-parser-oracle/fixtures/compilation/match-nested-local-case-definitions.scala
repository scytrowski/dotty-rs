object MatchCaseLayout {
  def enclosing = {
    def addFreeParams(proxies: List[Int]) = proxies match {
      case Nil => 0
      case values =>
        val count = values.size
        def result = count
        result
    }
    addFreeParams(Nil)
  }
}
