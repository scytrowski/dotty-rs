package chain
class TC[A]
object Public:
  export Middle.given
  def want[A](using e: TC[A]): TC[A] = e
  def inside: TC[Int] = want[Int]
object Rival:
  given other: TC[Int] = new TC[Int]
object Renamed:
  export Middle.{first as selected}
object Middle:
  export Leaf.{ints as first, given}
object Leaf:
  given ints: TC[Int] = new TC[Int]
  given flags: TC[Boolean] = new TC[Boolean]
class Client:
  def need[A](using e: TC[A]): TC[A] = e
  def control: TC[Int] = Middle.first
  def named: TC[Int] = Renamed.selected
  def all: TC[Boolean] = { import Public.given; need[Boolean] }
  def mono: TC[Int] = { import Public.given; need[Int] }
