trait T {
  val first = (
    foo match
        case A => a
        )
  def second(prefix: Int,
      values: List[Int]): Int = values match
    case B => b
  def after = 1
}
