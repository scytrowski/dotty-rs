package lexicalclosure

trait Parent:
  def inc(x: Int): Int = x + 1

class Child extends Parent:
  def throughSuper(delta: Int): Int => Int =
    x => super.inc(x) + delta

  def valid(delta: Int): Int => Int =
    x => x + delta
