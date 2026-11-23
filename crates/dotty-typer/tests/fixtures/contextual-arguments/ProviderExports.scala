package multi
class Key[A]
class Pair[A,B]
object Records:
  given mono(using k: Key[Int]): Pair[Int,Int] = new Pair[Int,Int]
object Mono:
  export Records.mono
class Client:
  def need[A](using x: A): A = x
  def directMono(using k: Key[Int]): Pair[Int,Int] = { import Records.mono; need[Pair[Int,Int]] }
  def viaMono(using k: Key[Int]): Pair[Int,Int] = { import Mono.given; need[Pair[Int,Int]] }
