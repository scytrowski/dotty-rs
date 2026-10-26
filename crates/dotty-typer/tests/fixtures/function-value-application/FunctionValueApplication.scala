import scala.annotation.tailrec

/** Pinned Scala 3.9 oracle for calls through existing function values. */
class IterableOnceFunctionRoots[A] {
  def foldl[X >: A, B](seq: IndexedSeq[X], start: Int, z: B, op: (B, X) => B): B = {
    @tailrec def loop(at: Int, end: Int, acc: B): B =
      if (at == end) acc
      else loop(at + 1, end, op(acc, seq(at)))
    loop(start, seq.length, z)
  }

  def foldr[X >: A, B >: X](seq: IndexedSeq[X], op: (X, B) => B): B = {
    @tailrec def loop(at: Int, acc: B): B =
      if (at == 0) acc
      else loop(at - 1, op(seq(at - 1), acc))
    loop(seq.length - 1, seq(seq.length - 1))
  }
}

object FunctionValueApplicationControls {
  def one(function: Int => String): String = function(42)

  def two(function: (Int, String) => String): String = function(42, "value")

  def generic[A, B](function: A => B, value: A): B = function(value)

  def nested(function: Int => Int, render: Int => String, value: Int): String =
    render(function(value))
}
