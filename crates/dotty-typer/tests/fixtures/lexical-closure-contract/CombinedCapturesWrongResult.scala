package lexicalclosure

class Calculator:
  def inc(x: Int): Int = x + 1
  def makeAdder(delta: Int): Int => Boolean =
    x => inc(x) + delta
