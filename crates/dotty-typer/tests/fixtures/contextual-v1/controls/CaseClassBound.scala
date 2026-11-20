trait TC[A]
case class Box[A: TC](value: A) {
  def evidence: TC[A] = summon[TC[A]]
}
class Client {
  def make(using leaf: TC[Int]): Box[Int] = Box(1)
}
