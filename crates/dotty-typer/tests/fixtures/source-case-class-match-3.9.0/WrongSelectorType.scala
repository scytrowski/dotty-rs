package matching
case class Box(value: Int)
object WrongSelectorType:
  def mismatch(box: Box): Int = box match
    case Box(value: String) => 0
