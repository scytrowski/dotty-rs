package trailingviews

trait TC[A]
class Box[A]
class Wrapped[A] { def show: A = ??? }
class Client {
  implicit def wrap[A](x: Box[A])(implicit tc: TC[A]): Wrapped[A] = ???
  def accept(x: Wrapped[Int]): Int = x.show
  def member(x: Box[Int])(using tc: TC[Int]): Int = x.show
  def argument(x: Box[Int])(using tc: TC[Int]): Int = accept(x)
}

class Parcel extends Box[Int]
object Parcel {
  implicit def fallback(x: Parcel): Wrapped[Int] = ???
}
class Fallback extends Client {
  def fallbackMember(x: Parcel): Int = x.show
  def fallbackArgument(x: Parcel): Int = accept(x)
}
