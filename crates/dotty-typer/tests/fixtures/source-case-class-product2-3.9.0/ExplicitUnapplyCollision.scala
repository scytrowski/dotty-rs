package matching
case class Flagged(value: Int, enabled: Boolean)
object Flagged:
  def unapply(item: Flagged): Option[(Int, Boolean)] = Some((item.value, item.enabled))
object ExplicitExtractor:
  def select(item: Flagged): Int = item match
    case Flagged(value, enabled) => value
