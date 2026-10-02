object NestedMatchLocalDefs:
  def select(input: Option[Int]) =
    val result = input match
      case Some(value) =>
        val selected = value match
          case 0 => 1
          case other => other
        def finish = selected
        finish
      case None => 0
    result
