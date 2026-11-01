package matching
case class Box(value: Int)

object OtherScope:
  object Box:
    def unapply(value: matching.Box): Option[Int] = Some(value.value)

  def extract(value: matching.Box): Int = value match
    case Box(result) => result
