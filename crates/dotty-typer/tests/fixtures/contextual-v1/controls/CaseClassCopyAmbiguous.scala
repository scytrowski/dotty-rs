trait TC[A]
case class Box[A: TC](value: A)
class Client {
  def duplicate(box: Box[Int])(using first: TC[Int], second: TC[Int]): Box[Int] = box.copy()
}
