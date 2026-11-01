package matching
case class Box(value: Int)
object Box:
  def unapply(value: Box): Int = value.value
object ExplicitUnapplyWrongResult:
  def extract(value: Box): Int = value match
    case Box(member) => member
