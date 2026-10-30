package lexicalclosure

class ByNameCapture:
  def capture(value: => Int): Int => Int =
    x => x + value

  def valid(delta: Int): Int => Int =
    x => x + delta
