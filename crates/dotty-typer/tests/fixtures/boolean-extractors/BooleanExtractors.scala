object Even:
  def unapply(value: Int): Boolean = value % 2 == 0

object BooleanExtractors:
  def classify(value: Int): String =
    value match
      case Even() => "even"
      case _ => "odd"
