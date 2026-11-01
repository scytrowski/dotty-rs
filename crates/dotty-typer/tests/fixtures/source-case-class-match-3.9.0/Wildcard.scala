package matching
case class Box(value: Int)
object Wildcard:
  def fallback(box: Box): Int = box match
    case _ => 1
