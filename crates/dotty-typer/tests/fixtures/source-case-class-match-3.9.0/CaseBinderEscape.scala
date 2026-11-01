package matching
case class Box(value: Int)
object CaseBinderEscape:
  def extract(box: Box): Int = box match
    case Box(value) => 0
    case _ => value
