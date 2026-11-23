package omitted
class Box[A]
class Fmt[A]
class Product[A]
class Pair[A, B]
trait Animal
class Dog extends Animal
class Anchor[A]
object Anchor:
  given chosen: Anchor[Boolean] = new Anchor[Boolean]
object Imports:
  given imported: Box[Boolean] = new Box[Boolean]
object Providers:
  given product[A](using b: Box[A], f: Fmt[A], flag: Boolean): Product[A] = new Product[A]
object Api:
  export Providers.{product as selected}
class Client:
  def one[A](using b: Box[A], flag: Boolean): Box[A] = b
  def read[A](using b: Box[A], f: Fmt[A], flag: Boolean): Box[A] = b
  def flip[A](using f: Fmt[A], b: Box[A], flag: Boolean): Box[A] = b
  def last[A](using flag: Boolean, b: Box[A]): Box[A] = b
  def pair[A,B](using b: Box[A], f: Fmt[B], flag: Boolean): Pair[A,B] = new Pair[A,B]
  def bounded[A <: Animal](using b: Box[A], flag: Boolean): Box[A] = b
  def anchor[A](using a: Anchor[A], flag: Boolean): Anchor[A] = a
  def old[A](implicit b: Box[A], f: Fmt[A], flag: Boolean): Box[A] = b
  def fallback[A](using b: Box[A], f: Fmt[A] = new Fmt[A], flag: Boolean): Box[A] = b
  def explicit(using b: Box[Int], f: Fmt[Int]): Any = read[Int](using flag = true)
  def written(b: Box[Int])(using f: Fmt[Int]): Any = read(using b = b, flag = true)
  def single(using b: Box[Int]): Any = one(using flag = true)
  def trailing(using b: Box[Boolean]): Any = last(using flag = true)
  def both(using b: Box[Int], f: Fmt[Int]): Any = read(using flag = true)
  def reverse(using b: Box[Int], f: Fmt[Int]): Any = flip(using flag = true)
  def twoVars(using b: Box[Int], f: Fmt[Boolean]): Any = pair(using flag = true)
  def partial(b: Box[Int])(using f: Fmt[Boolean]): Any = pair(using flag = true, b = b)
  def earlyFilter(using a: Box[Int], b: Box[Boolean], f: Fmt[Int]): Any = flip(using flag = true)
  def boundOk(using d: Box[Dog]): Any = bounded(using flag = true)
  def boundMix(using d: Box[Dog], i: Box[Int]): Any = bounded(using flag = true)
  def imported(): Any = { import Imports.given; one(using flag = true) }
  def companion(): Any = anchor(using flag = true)
  def exported(using b: Box[Int], f: Fmt[Int]): Any = Api.selected(using flag = true)
  def legacy(using b: Box[Int], f: Fmt[Int]): Any = old(using flag = true)
  def defaulted(using b: Box[Int]): Any = fallback(using flag = true)
  def shadow(using b: Box[Int], f: Fmt[Int]): Any = fallback(using flag = true)
