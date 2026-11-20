package conditionalviews

trait TC[A]
class Box[A]
class Wrapped[A] { def show: A = ??? }
class Client {
  given wrap[A](using tc: TC[A]): Conversion[Box[A], Wrapped[A]] = ???
  def converted(x: Box[Int])(using tc: TC[Int]): Int = x.show
}

class Alternate[A]
object Alternate {
  given fallback: Conversion[Alternate[Int], Wrapped[Int]] = ???
}
class Fallback {
  given conditional[A](using tc: TC[A]): Conversion[Alternate[A], Wrapped[A]] = ???
  def withoutEvidence(x: Alternate[Int]): Int = x.show
  def conflictingEvidence(x: Alternate[Int])(using first: TC[Int], second: TC[Int]): Int = x.show
}

class Mixed {
  implicit def legacy(x: Box[Int]): Wrapped[Int] = ???
  given modern[A](using tc: TC[A]): Conversion[Box[A], Wrapped[A]] = ???
  def mixed(x: Box[Int])(using tc: TC[Int]): Int = x.show
}

class Derived extends Client {
  implicit def inheritedAlternative(x: Box[Int]): Wrapped[Int] = ???
  def fromDerived(x: Box[Int])(using tc: TC[Int]): Int = x.show
}
