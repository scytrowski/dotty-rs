package example

class Algorithms:
  def add(a: Int, b: Int): Int = a + b
  def positive(x: Int): Boolean = x > 0
  def both(a: Boolean, b: Boolean): Boolean = a && b
  def safeAnd: Boolean = false && (1 / 0 > 0)
  def safeOr: Boolean = true || (1 / 0 > 0)
  def divisionByZero: Boolean = 1 / 0 > 0
  def sumTo(n: Int): Int =
    var i: Int = 0
    var total: Int = 0
    while i < n do
      total = total + i
      i = i + 1
    total
