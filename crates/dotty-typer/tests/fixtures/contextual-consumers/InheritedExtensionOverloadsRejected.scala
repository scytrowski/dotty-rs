package inheritedextensions
class Rejected extends Conflicting {
  def ambiguous(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2)
}
