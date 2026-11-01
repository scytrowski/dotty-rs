package matching
case class Foreign(value: Int, enabled: Boolean)
class Flagged(val value: Int, val enabled: Boolean)
object Flagged:
  def unapply(item: Foreign): Option[(Int, Boolean)] = Some((item.value, item.enabled))
object UnrelatedUse:
  def select(item: Foreign): Int = item match
    case Flagged(value, enabled) => value
