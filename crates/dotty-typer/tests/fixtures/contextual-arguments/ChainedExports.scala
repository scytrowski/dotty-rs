package chain
class TC[A]
class Box[A]
object Public:
  export Middle.given
  def want[A](using e: TC[A]): TC[A] = e
  def inside: TC[Int] = want[Int]
object Rival:
  given other: TC[Int] = new TC[Int]
object Conditional:
  export Middle.boxed
object Filtered:
  export Middle.given TC[Int]
object PlainStar:
  export Middle.*
object WithoutFirst:
  export Middle.{first as _, given}
object OldPublic:
  export OldMiddle.*
object OldMiddle:
  export Legacy.*
object Legacy:
  implicit val old: TC[Int] = new TC[Int]
object Renamed:
  export Middle.{first as selected}
object Middle:
  export Leaf.{ints as first, given}
object Leaf:
  given ints: TC[Int] = new TC[Int]
  given flags: TC[Boolean] = new TC[Boolean]
  given boxed[A](using e: TC[A]): TC[Box[A]] = new TC[Box[A]]
  private given hidden: TC[Long] = new TC[Long]
class Client:
  def need[A](using e: TC[A]): TC[A] = e
  def control: TC[Int] = Middle.first
  def named: TC[Int] = Renamed.selected
  def all: TC[Boolean] = { import Public.given; need[Boolean] }
  def mono: TC[Int] = { import Public.given; need[Int] }
  def bound: TC[Int] = { import Filtered.given; need[Int] }
  def legacy: TC[Int] = { import OldPublic.*; need[Int] }
  def retained: TC[Boolean] = { import WithoutFirst.given; need[Boolean] }
  def poly: TC[Box[Int]] = { import Public.given; need[Box[Int]] }
