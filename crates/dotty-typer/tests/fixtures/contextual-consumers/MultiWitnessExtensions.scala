package multiwitness
trait TC[A]
trait Format[B]
trait Trace[B]
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def keep[B](b: B)(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def keep(i: Int): Boolean
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def automatic(x: Box[Int])(using tc: TC[Int], f: Format[Long], t: Trace[Long]): Long = x.keep(2L)
  def explicit(x: Box[Int])(using tc: TC[Int], a: Format[Long], b: Format[Long], t: Trace[Long]): Long = x.keep(2L)(using b, t)
  def concrete(x: Box[Int])(using tc: TC[Int]): Boolean = x.keep(2)
  def expectedInt(x: Box[Int])(using tc: TC[Int], f: Format[Int], t: Trace[Int]): Int = x.keep(2)
}
