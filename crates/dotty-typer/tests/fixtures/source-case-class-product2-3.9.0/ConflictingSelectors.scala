package matching
case class Flagged(value: Int, enabled: Boolean):
  def _1: String = "conflict"
  def _2: String = "conflict"
object Conflict:
  def select(item: Flagged): Int = item match
    case Flagged(value, enabled) => value
