object TypedPatterns:
  def wildcard(value: Any): Int = value match
    case _: Int => 1

  def variable(value: Any): Int = value match
    case item: Int => item

  def explicit(value: Any): Int = value match
    case item @ (_: Int) => item
