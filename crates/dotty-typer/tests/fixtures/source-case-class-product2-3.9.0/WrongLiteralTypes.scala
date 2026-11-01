package matching
case class Flagged(value: Int, enabled: Boolean)
object BadLiteralTypes:
  def select(item: Flagged): Int = item match
    case Flagged(true, 1) => 0
    case _ => 1
