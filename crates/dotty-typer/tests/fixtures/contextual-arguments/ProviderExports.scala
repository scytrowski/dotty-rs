package multi
class Key[A]
class Tag[A]
class Pair[A,B]
object Records:
  given mono(using k: Key[Int]): Pair[Int,Int] = new Pair[Int,Int]
  given flat[A](using k: Key[A], t: Tag[A]): Pair[A,A] = new Pair[A,A]
  given poly[A,B](using k: Key[A], t: Tag[B]): Pair[A,B] = new Pair[A,B]
  given curried[A,B](using k: Key[A])(using t: Tag[B]): Pair[A,B] = new Pair[A,B]
object Mono:
  export Records.mono
object Flat:
  export Records.flat
object Poly:
  export Records.poly
object Curried:
  export Records.curried
class Client:
  def need[A](using x: A): A = x
  def directMono(using k: Key[Int]): Pair[Int,Int] = { import Records.mono; need[Pair[Int,Int]] }
  def directFlat(using k: Key[Int], t: Tag[Int]): Pair[Int,Int] = { import Records.flat; need[Pair[Int,Int]] }
  def directPoly(using k: Key[Int], t: Tag[Boolean]): Pair[Int,Boolean] = { import Records.poly; need[Pair[Int,Boolean]] }
  def directCurried(using k: Key[Int], t: Tag[Boolean]): Pair[Int,Boolean] = { import Records.curried; need[Pair[Int,Boolean]] }
  def viaMono(using k: Key[Int]): Pair[Int,Int] = { import Mono.given; need[Pair[Int,Int]] }
  def viaFlat(using k: Key[Int], t: Tag[Int]): Pair[Int,Int] = { import Flat.given; need[Pair[Int,Int]] }
  def viaPoly(using k: Key[Int], t: Tag[Boolean]): Pair[Int,Boolean] = { import Poly.given; need[Pair[Int,Boolean]] }
  def viaCurried(using k: Key[Int], t: Tag[Boolean]): Pair[Int,Boolean] = { import Curried.given; need[Pair[Int,Boolean]] }
