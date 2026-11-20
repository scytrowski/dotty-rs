package legacymembers

class Box[A] { def direct: Int = 1 }
class Wrapped[A] {
  def show: A = ???
  def direct: Int = 2
  private def hidden: Int = 0
}

object Box {
  implicit def wrap[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
}

class Client {
  def converted(x: Box[Int]): Int = x.show
  def keep(x: Box[Int]): Int = x.direct
}

class Local {
  implicit def local[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  implicit def ignore[A](x: Box[A]): Box[A] = x
  def fromScope(x: Box[Int]): Int = x.show
}

class Mixed {
  implicit def old[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  given modern: Conversion[Box[Int], Wrapped[Int]] = ???
  def mixed(x: Box[Int]): Int = x.show
}

class Specialized {
  implicit def generic[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  implicit def mono(x: Box[Int]): Wrapped[Int] = new Wrapped[Int]
  def specialized(x: Box[Int]): Int = x.show
}
