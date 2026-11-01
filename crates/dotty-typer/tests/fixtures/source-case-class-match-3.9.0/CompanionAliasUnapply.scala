package matching
case class Box(value: Int)
object Box:
  type Alias = Box
  def unapply(value: Alias): Option[Int] = Some(value.value)
object CompanionAliasUnapply:
  def extract(box: Box): Int = box match
    case Box(member) => member
