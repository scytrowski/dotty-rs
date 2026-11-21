package keepusing
trait TC[A]
trait Format[B]
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def keep[B](b: B)(using f: Format[B]): B
  extension (x: Box[A]) def keep(i: Int): Boolean
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def automatic(x: Box[Int])(using tc: TC[Int], f: Format[Long]): Long = x.keep(2L)
  def explicit(x: Box[Int])(using tc: TC[Int], a: Format[Long], b: Format[Long]): Long = x.keep(2L)(using b)
}
