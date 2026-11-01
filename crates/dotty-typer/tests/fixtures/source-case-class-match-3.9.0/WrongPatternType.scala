package matching
final case class Box(value: Int)
final case class OtherBox(value: Int)
object WrongPatternType:
  def extract(box: Box): Int = box match
    case OtherBox(value) => value
