package lexicalclosure

class Calculator:
  def inc(x: Int): Int = x + 1
  def makeAdder(delta: Int): Int => Int =
    x => inc(x) + delta
