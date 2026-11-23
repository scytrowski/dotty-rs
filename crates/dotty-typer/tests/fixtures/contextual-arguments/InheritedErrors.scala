package inheritexport
class Bad:
  def need[A](using e: TC[A]): TC[A] = e
  def clash: TC[Int] = { import Api.given; import Rival.given; need[Int] }
  def wrong: TC[Boolean] = { import Only.given; need[Boolean] }
  def hidden: TC[Long] = { import Api.given; need[Long] }
  def masked: TC[Int] = { import OldApi.*; need[Int] }
  def ranked: TC[Int] = { import Ranked.given; need[Int] }
