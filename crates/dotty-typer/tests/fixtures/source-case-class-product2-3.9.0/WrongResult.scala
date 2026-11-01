package matching
case class Flagged(value: Int, enabled: Boolean)
object BadResult:
  def select(item: Flagged): Int = item match
    case Flagged(_, _) => true
