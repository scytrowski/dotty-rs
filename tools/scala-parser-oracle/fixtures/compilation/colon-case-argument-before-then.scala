object ColonCaseArgumentBeforeThen:
  def check(values: List[Int]): Boolean =
    if values.forall:
      case _ => true
    then true
    else false
