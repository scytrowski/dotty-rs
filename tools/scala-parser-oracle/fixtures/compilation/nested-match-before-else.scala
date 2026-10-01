object NestedMatchBeforeElse:
  def setup(value: Option[Int], ready: Boolean) =
    if ready then
      value match
        case Some(x) =>
          val next = x
          next
        case None => 0
    else
      fallback()

  def after = 1
