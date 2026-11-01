package matching

case class Box(value: Int)

object Matcher:
  def unwrap(box: Box): Int =
    box match
      case Box(value) => value

  def classify(box: Box): Int =
    box match
      case Box(0) => 0
      case Box(value) => value
