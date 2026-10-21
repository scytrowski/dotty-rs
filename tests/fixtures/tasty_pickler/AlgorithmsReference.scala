package example

class AlgorithmsReference:
  def add(a: Int, b: Int): Int = a + b
  def subtract(a: Int, b: Int): Int = a - b
  def multiply(a: Int, b: Int): Int = a * b
  def divide(a: Int, b: Int): Int = a / b
  def remainder(a: Int, b: Int): Int = a % b
  def less(a: Int, b: Int): Boolean = a < b
  def lessOrEqual(a: Int, b: Int): Boolean = a <= b
  def greater(a: Int, b: Int): Boolean = a > b
  def greaterOrEqual(a: Int, b: Int): Boolean = a >= b
  def positive(x: Int): Boolean = x > 0
  def negate(value: Boolean): Boolean = !value
  def both(a: Boolean, b: Boolean): Boolean = a && b
  def either(a: Boolean, b: Boolean): Boolean = a || b
  def eagerAnd(a: Boolean, b: Boolean): Boolean = a & b
  def eagerOr(a: Boolean, b: Boolean): Boolean = a | b
  def exclusiveOr(a: Boolean, b: Boolean): Boolean = a ^ b
  def shortCircuitAnd: Boolean = false && (1 / 0 > 0)
  def shortCircuitOr: Boolean = true || (1 / 0 > 0)
  def safeAnd: Boolean = false && (1 / 0 > 0)
  def safeOr: Boolean = true || (1 / 0 > 0)
  def divisionByZero: Boolean = 1 / 0 > 0
  def eagerAndEffect: Boolean = false & (1 / 0 > 0)
  def eagerOrEffect: Boolean = true | (1 / 0 > 0)
  def immutableLocal(value: Int): Int =
    val saved: Int = value
    saved
  def sumTo(n: Int): Int =
    var i: Int = 0
    var total: Int = 0
    while i < n do
      total = total + i
      i = i + 1
    total
