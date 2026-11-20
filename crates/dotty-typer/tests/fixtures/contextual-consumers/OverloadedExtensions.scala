package extnext
trait TC[A]
trait Format
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def render(i: Int)(using fmt: Format): A
  extension (x: Box[A]) def render(b: Boolean): Boolean
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def probeInt(x: Box[Int])(using tc: TC[Int], fmt: Format): Int = x.render(3)
  def probeBoolean(x: Box[Int])(using tc: TC[Int]): Boolean = x.render(true)
}

class Convertible extends Box[Int]
class LongOps { def render(i: Long): Long = i }
object Convertible {
  given view: Conversion[Convertible, LongOps] = ???
}
class Alternative extends Client {
  def probeConversion(x: Convertible)(using tc: TC[Int]): Long = x.render(3L)
}
