package matching
case class Flagged(value: Int, enabled: Boolean)
object BadArityThree:
  def select(item: Flagged): Int = item match
    case Flagged(value, enabled, extra) => value
