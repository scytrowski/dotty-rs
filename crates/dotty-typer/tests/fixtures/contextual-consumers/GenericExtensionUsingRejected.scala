package keepusing
class Rejected extends Client {
  def missing(x: Box[Int])(using tc: TC[Int]): Long = x.keep(2L)
  def ambiguous(x: Box[Int])(using tc: TC[Int], a: Format[Long], b: Format[Long]): Long = x.keep(2L)
}
