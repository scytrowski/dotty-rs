trait TC[A]
case class Box[A: TC](value: A)
class Client {
  def ambiguous(using first: TC[Int], second: TC[Int]): Box[Int] = Box(1)
}
