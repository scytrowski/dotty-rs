package matching
class Ordinary(val value: Int, val enabled: Boolean)
object OrdinaryUse:
  def select(item: Ordinary): Int = item match
    case Ordinary(value, enabled) => value
