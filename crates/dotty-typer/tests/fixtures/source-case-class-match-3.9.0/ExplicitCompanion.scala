package matching
case class Box(value: Int)
object Box:
  def marker: Int = 1
object ExplicitCompanion:
  def extract(box: Box): Int = box match
    case Box(value) => value
