trait TC[A]
class Box[A: TC]
class Client {
  def missing: Box[Int] = new Box[Int]
}
