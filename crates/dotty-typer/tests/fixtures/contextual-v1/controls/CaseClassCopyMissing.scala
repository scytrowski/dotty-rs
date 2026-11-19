trait TC[A]
case class Box[A: TC](value: A)
class Client {
  def duplicate(box: Box[Int]): Box[Int] = box.copy()
}
