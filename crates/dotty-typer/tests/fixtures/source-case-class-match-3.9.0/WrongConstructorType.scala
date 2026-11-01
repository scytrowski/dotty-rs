package matching
case class Box(value: Int)
class OtherBox
object WrongConstructorType:
  def mismatch(box: OtherBox): Int = box match
    case Box(value) => value
