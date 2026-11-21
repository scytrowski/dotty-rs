package groupusing
class Failed extends Client {
  def probeOne(x: Narrow[Int])(using tc: TC[Int], c: Compact): Int = x.render(3)
  def probeBoth(x: Narrow[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = x.render(3)
  def probeWrong(x: Narrow[Int])(using tc: TC[Int]): Int = x.render(3)(using true)
}
