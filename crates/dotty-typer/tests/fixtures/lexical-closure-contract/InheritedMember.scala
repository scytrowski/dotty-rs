package lexicalclosure

trait Parent:
  def inc(x: Int): Int = x + 1

class Child extends Parent:
  def inherited(delta: Int): Int => Int =
    x => inc(x) + delta

  def valid(delta: Int): Int => Int =
    x => x + delta
