package next
trait TC[A]
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def keep[B](b: B): B
  extension (x: Box[A]) def keep(i: Int): Boolean
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def probeGeneric(x: Box[Int])(using tc: TC[Int]): Long = x.keep(2L)
  def wideResult(x: Box[Int])(using tc: TC[Int]): Any = x.keep(2)
  def expectedInt(x: Box[Int])(using tc: TC[Int]): Int = x.keep(2)
  def probeSpecific(x: Box[Int])(using tc: TC[Int]): Boolean = x.keep(2)
}
