package extnext
class Failed extends Client {
  def probeMissingFormat(x: Box[Int])(using tc: TC[Int]): Int = x.render(3)
  def probeNoOverload(x: Box[Int])(using tc: TC[Int]): Int = x.render(3L)
}
