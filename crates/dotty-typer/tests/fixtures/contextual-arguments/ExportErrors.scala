package exportsprint
class Bad:
  def need[A](using e: TC[A]): TC[A] = e
  def excluded: TC[Boolean] = { import Only.given; need[Boolean] }
  def star: TC[Int] = { import Star.given; need[Int] }
  def hidden: TC[Long] = { import All.given; need[Long] }
  def ambiguous: TC[Int] = { import All.given; import Other.given; need[Int] }
