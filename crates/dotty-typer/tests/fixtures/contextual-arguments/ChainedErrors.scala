package chain
class Bad:
  def ask[A](using e: TC[A]): TC[A] = e
  def clash: TC[Int] = { import Public.given; import Rival.given; ask[Int] }
