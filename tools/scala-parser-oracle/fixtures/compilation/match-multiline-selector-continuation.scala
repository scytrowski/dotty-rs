object MatchMultilineSelectorContinuation:
  def valueOf(value: Int) =
    java.lang.Integer.valueOf(value)
    match
      case number if number == 0 => 0
      case number => number
