package boundedwitness
trait Base
class Leaf extends Base
class Other
trait TC[A]
trait Format[B]
trait Trace[B]
class Box[A]
trait Ops[A] {
  extension (x: Box[A]) def keep[B <: Base](b: B)(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def keep(i: Int): Boolean
  extension (x: Box[A]) def wide[B <: Base](b: B)(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def wide(b: Any): Boolean
  extension (x: Box[A]) def only[B <: Base](b: B)(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def take[B <: Base](b: Box[B])(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def take(i: Int): Boolean
  extension (x: Box[A]) def takeWide[B <: Base](b: Box[B])(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def takeWide(b: Any): Boolean
  extension (x: Box[A]) def exact[B <: Base](b: Box[B])(using f: Format[B], t: Trace[B]): B
  extension (x: Box[A]) def exact(b: Box[Leaf]): Boolean
}
class Client {
  given ops[A](using tc: TC[A]): Ops[A] = ???
  def leaf(x: Box[Int], b: Leaf)(using tc: TC[Int], f: Format[Leaf], t: Trace[Leaf]): Leaf = x.keep(b)
  def concrete(x: Box[Int])(using tc: TC[Int]): Boolean = x.keep(2)
  def overlap(x: Box[Int], b: Leaf)(using tc: TC[Int], f: Format[Leaf], t: Trace[Leaf]): Any = x.wide(b)
  def nested(x: Box[Int], b: Box[Leaf])(using tc: TC[Int], f: Format[Leaf], t: Trace[Leaf]): Leaf = x.take(b)
  def nestedExplicit(x: Box[Int], b: Box[Leaf])(using tc: TC[Int], f: Format[Leaf], t: Trace[Leaf]): Leaf = x.take(b)(using f, t)
  def nestedConcrete(x: Box[Int])(using tc: TC[Int]): Boolean = x.take(2)
  def nestedOverlap(x: Box[Int], b: Box[Leaf])(using tc: TC[Int], f: Format[Leaf], t: Trace[Leaf]): Any = x.takeWide(b)
  def nestedExact(x: Box[Int], b: Box[Leaf])(using tc: TC[Int]): Any = x.exact(b)
}
