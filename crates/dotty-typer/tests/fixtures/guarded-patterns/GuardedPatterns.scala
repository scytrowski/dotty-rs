object GuardedPatterns:
  val Stable: Int = 1

  def predicate(value: Int): Boolean = true

  def variable(value: Int): Int = value match
    case item if predicate(item) => item

  def explicit(value: Int): Int = value match
    case item @ _ if predicate(item) => item

  def literal(value: Int): Int = value match
    case 1 if true => 1

  def stable(value: Int): Int = value match
    case Stable if true => 1

  def wildcard(value: Int): Int = value match
    case _ if true => 1
