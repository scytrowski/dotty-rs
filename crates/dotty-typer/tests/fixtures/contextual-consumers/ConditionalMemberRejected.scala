package conditionalviews

class Rejected extends Client {
  def missing(x: Box[Int]): Int = x.show
  def ambiguous(x: Box[Int])(using first: TC[Int], second: TC[Int]): Int = x.show
}
