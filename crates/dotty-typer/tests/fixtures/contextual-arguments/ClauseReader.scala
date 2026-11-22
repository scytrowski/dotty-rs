package defaults

class ClauseReader:
  def fold(using seed: Int)(x: Int)(y: Int = seed, z: Int = x): Int = y
  def finish(using seed: Int)(x: Int)()(y: Int = seed)(using flag: Boolean): Int = y
  def legacy(x: Int)(y: Int)(implicit n: Int = y): Int = n
  def startsPlain(x: Int = 1)(using seed: Int): Int = x
