package matching
case class Flagged(value: Int, enabled: Boolean)
object BadArityOne:
  def select(item: Flagged): Int = item match
    case Flagged(value) => value
