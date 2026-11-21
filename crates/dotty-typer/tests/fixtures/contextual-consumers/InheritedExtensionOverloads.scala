package inheritedextensions
trait TC[A]
class Box[A]
class Narrow[A] extends Box[A]
trait GenericBase[A] {
  extension (x: Narrow[A]) def keep[B](value: B): B
}
trait ConcreteOps[A] extends GenericBase[A] {
  extension (x: Box[A]) def keep(i: Int): Boolean
}
trait ConcreteBase[A] {
  extension (x: Box[A]) def keep(i: Int): Boolean
}
trait GenericOps[A] extends ConcreteBase[A] {
  extension (x: Narrow[A]) def keep[B](value: B): B
}
class DerivedConcrete {
  given concrete[A](using tc: TC[A]): ConcreteOps[A] = ???
  def concreteInt(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2)
  def inheritedLong(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2L)
}
class DerivedGeneric {
  given generic[A](using tc: TC[A]): GenericOps[A] = ???
  def genericInt(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2)
  def genericLong(x: Narrow[Int])(using tc: TC[Int]): Any = x.keep(2L)
}
trait ConcreteNarrowBase[A] {
  extension (x: Narrow[A]) def keep(i: Int): Boolean
}
trait WideGenericOps[A] extends ConcreteNarrowBase[A] {
  extension (x: Box[A]) def keep[B](value: B): B
}
class Conflicting {
  given wide[A](using tc: TC[A]): WideGenericOps[A] = ???
  def expectedBoolean(x: Narrow[Int])(using tc: TC[Int]): Boolean = x.keep(2)
  def expectedInt(x: Narrow[Int])(using tc: TC[Int]): Int = x.keep(2)
}
