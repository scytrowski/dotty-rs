trait TC[A]
class Box[A: TC] {
  def evidence: TC[A] = summon[TC[A]]
}
class Client {
  def make(using leaf: TC[Int]): Box[Int] = new Box[Int]
}
