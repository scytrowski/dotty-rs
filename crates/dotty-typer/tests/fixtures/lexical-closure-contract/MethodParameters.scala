package lexicalclosure

object MethodParameters:
  def addBy(delta: Int): Int => Int =
    x => x + delta

  def addBoth(first: Int, second: Int): Int => Int =
    x => x + first + second + first

  def shadow(delta: Int): Int => Int =
    (delta: Int) => delta

  def byName(delta: => Int): Int => Int =
    x => x + delta
