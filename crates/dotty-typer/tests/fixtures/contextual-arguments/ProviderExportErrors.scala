package multi
class Bad:
  def need[A](using x: A): A = x
  def missingFirst(using t: Tag[Boolean]): Pair[Int,Boolean] = { import Curried.given; need[Pair[Int,Boolean]] }
  def missingSecond(using k: Key[Int]): Pair[Int,Boolean] = { import Curried.given; need[Pair[Int,Boolean]] }
  def clashSecond(using k: Key[Int], left: Tag[Boolean], right: Tag[Boolean]): Pair[Int,Boolean] = { import Again.given; need[Pair[Int,Boolean]] }
  def directMissingFirst(using t: Tag[Boolean]): Pair[Int,Boolean] = { import Records.curried; need[Pair[Int,Boolean]] }
  def directMissingSecond(using k: Key[Int]): Pair[Int,Boolean] = { import Records.curried; need[Pair[Int,Boolean]] }
  def directClashSecond(using k: Key[Int], left: Tag[Boolean], right: Tag[Boolean]): Pair[Int,Boolean] = { import Records.curried; need[Pair[Int,Boolean]] }
