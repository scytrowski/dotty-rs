package sameargs
class Failed extends Client {
  def probeOne(x: Box[Int])(using tc: TC[Int], c: Compact): Int = x.render(3)
  def probeBoth(x: Box[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = x.render(3)
  def probeWrong(x: Box[Int])(using tc: TC[Int]): Int = x.render(3)(using true)
}
