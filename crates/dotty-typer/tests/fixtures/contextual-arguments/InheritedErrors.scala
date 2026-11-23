package inheritexport
class Bad:
  def need[A](using e: TC[A]): TC[A] = e
  def clash: TC[Int] = { import Api.given; import Rival.given; need[Int] }
  def ranked: TC[Int] = { import Ranked.given; need[Int] }
