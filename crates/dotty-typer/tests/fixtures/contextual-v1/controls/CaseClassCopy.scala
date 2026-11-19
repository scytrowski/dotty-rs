trait TC[A]
case class Box[A: TC](value: A)
class Client {
  def original(initial: TC[Int]): Box[Int] = Box(1)(using initial)
  def duplicate(box: Box[Int])(using current: TC[Int]): Box[Int] = box.copy()
}
