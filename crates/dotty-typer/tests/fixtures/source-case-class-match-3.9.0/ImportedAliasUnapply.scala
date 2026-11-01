package matching
case class Box(value: Int)
object ExtractorTypes:
  type Alias = Box
import ExtractorTypes.Alias
object Box:
  def unapply(value: Alias): Option[Int] = Some(value.value)
object ImportedAliasUnapply:
  def extract(box: Box): Int = box match
    case Box(member) => member
