object NestedBracedMatchBlockTail:
  def check(value: Option[Int]): Boolean =
    val result = value match {
      case Some(number) => number > 0
      case None => false
    }
    !result
