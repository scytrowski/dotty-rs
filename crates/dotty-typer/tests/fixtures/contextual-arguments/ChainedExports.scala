package chain
class TC[A]
object Renamed:
  export Middle.{first as selected}
object Middle:
  export Leaf.{ints as first, given}
object Leaf:
  given ints: TC[Int] = new TC[Int]
class Client:
  def need[A](using e: TC[A]): TC[A] = e
  def control: TC[Int] = Middle.first
  def named: TC[Int] = Renamed.selected
