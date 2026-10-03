object VariablePatterns:
  def classify(value: Int): Int = value match
    case item => item

  def explicit(value: Int): Int = value match
    case item @ _ => item

  def shadow(value: Any): Any = value match
    case value => value
