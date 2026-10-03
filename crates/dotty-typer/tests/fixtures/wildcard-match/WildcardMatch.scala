object WildcardMatch:
  def simple(value: Int): Int = value match
    case _ => 1

  def multiple(value: Int): AnyVal = value match
    case _ => 1
    case _ => true

  def nested(value: Int, other: Int): Int = value match
    case _ => other match
      case _ => 1
