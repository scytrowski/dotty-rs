package multiwitness
class Rejected extends Client {
  def missing(x: Box[Int])(using tc: TC[Int], f: Format[Long]): Long = x.keep(2L)
  def ambiguous(x: Box[Int])(using tc: TC[Int], f: Format[Long], a: Trace[Long], b: Trace[Long]): Long = x.keep(2L)
}
