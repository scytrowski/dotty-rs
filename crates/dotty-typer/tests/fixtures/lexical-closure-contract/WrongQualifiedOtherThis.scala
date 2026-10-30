package lexicalclosure

class Outer:
  class Other:
    def value: Int = 1

  def wrong(delta: Int): Int => Int =
    x => Other.this.value + delta

  def valid(delta: Int): Int => Int =
    x => x + delta
