object PatternAlternatives {
  final val First = 1
  final val Second = 2

  def literal(x: Int): Int = x match {
    case 1 | 2 => 10
    case _ => 0
  }

  def stable(x: Int): Int = x match {
    case First | Second => 10
    case _ => 0
  }
}
