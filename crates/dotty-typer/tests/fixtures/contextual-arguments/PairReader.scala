package defaults

class PairReader:
  def pick(x: Int, y: Int)(using n: Int = y): Int = n
  def choose(using seed: Int)(value: Int = seed): Int = value
  def legacy(x: Int, y: Int)(implicit n: Int = y): Int = n
