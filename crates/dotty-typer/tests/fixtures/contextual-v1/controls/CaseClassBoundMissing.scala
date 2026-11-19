trait TC[A]
case class Box[A: TC](value: A)
class Client {
  def missing: Box[Int] = Box(1)
}
