package matching
case class Box(value: Int)
type Alias = Box
object Box:
  def unapply(value: Alias): Option[Int] = Some(value.value)
object PackageAliasUnapply:
  def extract(box: Box): Int = box match
    case Box(member) => member
