package matching
case class Box(value: Int)
object WrongArityTwo:
  def mismatch(box: Box): Int = box match
    case Box(a, b) => a + b
