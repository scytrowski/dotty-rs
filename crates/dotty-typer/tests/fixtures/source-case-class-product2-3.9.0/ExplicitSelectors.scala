package matching
case class Flagged(value: Int, enabled: Boolean):
  def _1: String = value.toString
  def _2: String = enabled.toString
object ExplicitSelectors:
  def select(item: Flagged): String = item match
    case Flagged(value, enabled) => if enabled.nonEmpty then value else ""
