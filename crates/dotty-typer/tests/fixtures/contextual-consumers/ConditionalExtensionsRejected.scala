package conditionalext

class Rejected extends Client {
  def missing(x: Box[Int]): Int = x.render(3)
  def ambiguous(x: Box[Int])(using first: TC[Int], second: TC[Int]): Int = x.render(3)
}

class RejectedFormat extends FormattedClient {
  def noProvider(x: Box[Int])(using fmt: Format): Int = x.render(3)
  def noFormat(x: Box[Int])(using tc: TC[Int]): Int = x.render(3)
  def twoFormats(x: Box[Int])(using tc: TC[Int], first: Format, second: Format): Int = x.render(3)
}
