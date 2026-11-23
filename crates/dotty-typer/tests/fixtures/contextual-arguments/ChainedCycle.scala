package cycles
class TC[A]
object LeftSide:
  export RightSide.given
object RightSide:
  export LeftSide.given
class Client:
  def need[A](using e: TC[A]): TC[A] = e
  def use: TC[Int] = { import LeftSide.given; need[Int] }
