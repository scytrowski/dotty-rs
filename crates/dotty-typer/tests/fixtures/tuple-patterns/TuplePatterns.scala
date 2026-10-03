object TuplePatterns {
  def any(value: Any): Any = value match {
    case (first, second) => first
    case _ => value
  }

  def nested(value: Any): Any = value match {
    case (1, (first, second), third) => second
    case _ => value
  }

  def transparent(value: Any): Any = value match {
    case (single) => single
    case _ => value
  }

  def unit(value: Any): Int = value match {
    case () => 1
    case _ => 0
  }
}
