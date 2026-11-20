package next
class Rejected extends Client {
  def wrongArity(x: Box[Int])(using tc: TC[Int]): Long = x.keep(2L, 3L)
}
