object ValuePatterns:
  val Stable: Int = 1

  object Nested:
    val selected: Int = 2

  def literal(value: Int): Int = value match
    case 1 => 1

  def constantSelector: Int = 1 match
    case 1 => 1

  def boolean(value: Boolean): Int = value match
    case true => 1

  def stable(value: Int): Int = value match
    case Stable => 1

  def selected(value: Int): Int = value match
    case Nested.selected => 1

  def bindLiteral(value: Int): Int = value match
    case bound @ 1 => bound

  def bindStable(value: Int): Int = value match
    case bound @ Stable => bound
