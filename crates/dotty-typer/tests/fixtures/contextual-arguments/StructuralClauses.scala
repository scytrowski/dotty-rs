package structuralclauses

class Box[A]
trait Mark[A]
object Makers {
  given simple[A]: Box[A] with {}
  given mono(using seed: Int): Mark[Int] with {}
}
class Client {
  import Makers.given
  def box(using b: Box[Int]): Box[Int] = b
  def mark(using m: Mark[Int]): Mark[Int] = m
  def typeOnly: Box[Int] = box
  def contextOnly(using seed: Int): Mark[Int] = mark
}
