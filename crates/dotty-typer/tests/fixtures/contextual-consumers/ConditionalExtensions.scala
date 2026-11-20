package conditionalext

trait TC[A]
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def render(i: Int): A
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def selected(x: Box[Int])(using tc: TC[Int]): Int = x.render(3)
}

class Parcel extends Box[Int]
object Parcel {
  given fallback: Ops[Int] = ???
}
class Converted extends Box[Int]
class Wrapped { def render(i: Int): Int = i }
object Converted {
  given view: Conversion[Converted, Wrapped] = ???
}
class Alternatives extends Client {
  def fromCompanion(x: Parcel): Int = x.render(3)
  def fromConversion(x: Converted)(using first: TC[Int], second: TC[Int]): Int = x.render(3)
}
