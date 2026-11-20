package conditionalext

class Rejected extends Client {
  def missing(x: Box[Int]): Int = x.render(3)
  def ambiguous(x: Box[Int])(using first: TC[Int], second: TC[Int]): Int = x.render(3)
}
