package groups
trait TC[A]
class Box[A]
class Narrow[A] extends Box[A]
trait Ops[A] {
  extension (x: Narrow[A]) def keep(i: Int): Int
  extension (x: Narrow[A]) def keep(b: Boolean): Boolean
  extension (x: Box[A]) def keep[B](value: B): B
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def narrowInt(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2)
  def narrowBool(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(true)
  def narrowLong(x: Narrow[Int])(using tc: TC[Int]): Long = x.keep(2L)
  def boxInt(x: Box[Int])(using tc: TC[Int]): Any = x.keep(2)
}
