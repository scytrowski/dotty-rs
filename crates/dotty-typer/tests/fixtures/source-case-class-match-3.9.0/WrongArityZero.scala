package matching
case class Box(value: Int)
object WrongArityZero:
  def mismatch(box: Box): Int = box match
    case Box() => 0
