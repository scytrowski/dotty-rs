package matching
case class Box(value: Int)
object Box:
  def unapply(value: Box): Option[Int] = Some(value.value)
object ExplicitUnapplyReuse:
  def extract(value: Box): Int = value match
    case Box(member) => member
