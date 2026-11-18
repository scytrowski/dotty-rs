class Invalid:
  def need(using n: Int): Int = n
  def empty(using n: Int): Int = need()
  def missing: Int = need
  def ambiguous: Int =
    given one: Int = 1
    given two: Int = 2
    need
  def absent(using n: scala.util.NotGiven[Int]): Int = 1
  def present(using n: Int): Int = absent
