package lexicalclosure

object Mutable:
  def mutableCapture: Int => Int =
    var delta = 2
    x => x + delta
