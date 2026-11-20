trait TC[A]
class Box[A: TC]
class Client {
  def ambiguous(using first: TC[Int], second: TC[Int]): Box[Int] = new Box[Int]
}
