object MatchCaseGuardInfixPrefixNot:
  def f(x: Int): Int = x match
    case n if n > 0
    || !false => 1
    case _ => 0
  val after = 2
