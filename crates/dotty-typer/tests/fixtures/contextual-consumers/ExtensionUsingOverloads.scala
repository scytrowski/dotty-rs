package sameargs
trait TC[A]
trait Compact
trait Detailed
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def render(i: Int)(using c: Compact): A
  extension (x: Box[A]) def render(i: Int)(using d: Detailed): A
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def compact(x: Box[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = x.render(3)(using c)
  def parenthesized(x: Box[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = (x.render(3))(using c)
  def detailed(x: Box[Int])(using tc: TC[Int], c: Compact, d: Detailed): Int = x.render(3)(using d)
}
