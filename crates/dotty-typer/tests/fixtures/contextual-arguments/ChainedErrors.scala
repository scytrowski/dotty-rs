package chain
class Bad:
  def ask[A](using e: TC[A]): TC[A] = e
  def clash: TC[Int] = { import Public.given; import Rival.given; ask[Int] }
  def excluded: TC[Boolean] = { import Filtered.given; ask[Boolean] }
  def modernStar: TC[Int] = { import PlainStar.given; ask[Int] }
  def hidden: TC[Long] = { import Public.given; ask[Long] }
  def omitted: TC[Int] = { import WithoutFirst.given; ask[Int] }
