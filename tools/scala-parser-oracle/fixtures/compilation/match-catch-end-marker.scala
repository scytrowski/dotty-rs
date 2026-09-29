object MatchCatchEndMarker:
  def foldBinop(value: Int): Int =
    val optag = value
    try optag match
      case 0 => 0
      case 1 => 1
      case _ => 2
      catch case ex: ArithmeticException => 0 // preserve a safe fallback
  end foldBinop
end MatchCatchEndMarker
