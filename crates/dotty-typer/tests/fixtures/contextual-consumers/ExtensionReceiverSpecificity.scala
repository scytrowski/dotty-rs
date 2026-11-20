package receivers
trait TC[A]
class Box[A]
class Narrow[A] extends Box[A]
trait Ops[A] {
  extension (x: Narrow[A]) def keep[B](b: B): B
  extension (x: Box[A]) def keep(i: Int): Boolean
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def probeGeneric(x: Narrow[Int])(using tc: TC[Int]): Long = x.keep(2L)
  def probeSpecific(x: Narrow[Int])(using tc: TC[Int]): Int = x.keep(2)
  def probeWideResult(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2)
  def probeBox(x: Box[Int])(using tc: TC[Int]): Boolean = x.keep(2)
}
trait ReverseOps[A] {
  extension (x: Box[A]) def keep[B](b: B): B
  extension (x: Narrow[A]) def keep(i: Int): Boolean
}
class ReverseClient {
  given reverse[A](using tc: TC[A]): ReverseOps[A] = ???
  def narrowConcrete(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2)
  def baseGeneric(x: Box[Int])(using tc: TC[Int]): Int = x.keep(2)
}
