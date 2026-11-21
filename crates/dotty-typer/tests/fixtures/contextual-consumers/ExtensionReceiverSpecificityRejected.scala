package receivers
class Negative extends Client {
  def wrongResult(x: Narrow[Int])(using tc: TC[Int]): Boolean = x.keep(2)
}
class ReverseNegative extends ReverseClient {
  def wrongArgument(x: Narrow[Int])(using tc: TC[Int]): Long = x.keep(2L)
}
