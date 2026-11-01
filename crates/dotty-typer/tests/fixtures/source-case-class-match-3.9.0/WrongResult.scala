package matching
case class Box(value: Int)
object WrongResult:
  def mismatch(box: Box): Boolean = box match
    case Box(value) => value
