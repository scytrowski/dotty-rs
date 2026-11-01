package matching
case class Reversed(enabled: Boolean, value: Int)
object ReversedPattern:
  def inspect(item: Reversed): Boolean = item match
    case Reversed(first, second) => first
