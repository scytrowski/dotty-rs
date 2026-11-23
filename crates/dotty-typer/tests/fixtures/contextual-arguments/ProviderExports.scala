package multi
class Key[A]
class Tag[A]
class Pair[A,B]
object Records:
  given mono(using k: Key[Int]): Pair[Int,Int] = new Pair[Int,Int]
  given flat[A](using k: Key[A], t: Tag[A]): Pair[A,A] = new Pair[A,A]
object Mono:
  export Records.mono
object Flat:
  export Records.flat
class Client:
  def need[A](using x: A): A = x
  def directMono(using k: Key[Int]): Pair[Int,Int] = { import Records.mono; need[Pair[Int,Int]] }
  def directFlat(using k: Key[Int], t: Tag[Int]): Pair[Int,Int] = { import Records.flat; need[Pair[Int,Int]] }
  def viaMono(using k: Key[Int]): Pair[Int,Int] = { import Mono.given; need[Pair[Int,Int]] }
  def viaFlat(using k: Key[Int], t: Tag[Int]): Pair[Int,Int] = { import Flat.given; need[Pair[Int,Int]] }
