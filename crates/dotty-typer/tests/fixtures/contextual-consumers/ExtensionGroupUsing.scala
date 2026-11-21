package groupusing
trait TC[A]
trait Compact
trait Detailed
class Box[A]
class Narrow[A] extends Box[A]
trait Ops[A] {
  extension (x: Narrow[A]) def render(i: Int)(using c: Compact): A
  extension (x: Narrow[A]) def render(i: Int)(using d: Detailed): A
  extension (x: Box[A]) def render[B](value: B): B
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def compact(x: Narrow[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = x.render(3)(using c)
  def parenthesized(x: Narrow[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = (x.render(3))(using c)
  def detailed(x: Narrow[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = x.render(3)(using d)
  def wideFallback(x: Narrow[Int])(using tc: TC[Int]): Long = x.render(2L)
}
