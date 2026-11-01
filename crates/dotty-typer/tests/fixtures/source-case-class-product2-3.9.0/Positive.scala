package matching

case class Flagged(value: Int, enabled: Boolean)

object Matcher:
  def select(item: Flagged): Int =
    item match
      case Flagged(value, enabled) =>
        if enabled then value else 0

  def classify(item: Flagged): Int =
    item match
      case Flagged(0, true) => 0
      case Flagged(value, _) => value
