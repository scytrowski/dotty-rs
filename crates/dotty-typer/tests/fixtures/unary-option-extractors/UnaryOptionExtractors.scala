object SomeInt {
  def unapply(value: Any): Option[Int] = Some(1)
}

object Outer {
  def unapply(value: Any): Option[Any] = Some(value)
}

object UnaryOptionExtractors {
  def wildcard(value: Any): Int = value match {
    case SomeInt(_) => 1
    case _ => 0
  }

  def binding(value: Any): Int = value match {
    case SomeInt(x) if x > 0 => x
    case _ => 0
  }

  def literal(value: Any): Int = value match {
    case SomeInt(1) => 1
    case _ => 0
  }

  def typed(value: Any): Int = value match {
    case SomeInt(x: Int) => x
    case _ => 0
  }

  def nested(value: Any): Int = value match {
    case Outer(SomeInt(x)) => x
    case _ => 0
  }
}
