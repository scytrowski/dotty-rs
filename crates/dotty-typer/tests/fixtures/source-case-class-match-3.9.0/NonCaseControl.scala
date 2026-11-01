package matching
class Box(val value: Int)
object Box:
  def unapply(box: Box): Option[Int] = Some(box.value)
object NonCaseControl:
  def extract(box: Box): Int = box match
    case Box(value) => value
