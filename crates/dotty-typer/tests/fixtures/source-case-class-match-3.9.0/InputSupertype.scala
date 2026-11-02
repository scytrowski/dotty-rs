package matching

case class Box(value: Int)

object AnyExtractor:
  def unapply(value: Any): Option[Int] = Some(1)

object InputSupertype:
  def select(value: Box): Int = value match
    case AnyExtractor(extracted) => extracted
