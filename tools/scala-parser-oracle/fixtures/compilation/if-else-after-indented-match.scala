object IfElseAfterIndentedMatch:
  def choose(flag: Boolean, value: Int): Int =
    if flag then
      value match
        case 0 => 1
        case _ => 2
    else 3
