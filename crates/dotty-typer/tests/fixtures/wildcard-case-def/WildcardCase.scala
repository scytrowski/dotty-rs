object WildcardCase:
  def classify(value: Int): Int = value match
    case _ => 1
