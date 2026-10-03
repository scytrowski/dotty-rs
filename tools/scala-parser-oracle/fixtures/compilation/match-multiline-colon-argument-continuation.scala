object MatchMultilineColonArgumentContinuation:
  def firstPositive(values: List[Int]) =
    values.find: value =>
      value > 0
    match
      case Some(value) => value
      case None => 0
