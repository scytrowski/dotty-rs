class Calculator:
  def sumTo(n: Int): Int =
    var i = 1
    var sum = 0
    while i <= n do
      sum = sum + i
      i = i + 1
    sum

  def add(a: Int, b: Int): Int = a + b
  def subtract(a: Int, b: Int): Int = a - b
  def multiply(a: Int, b: Int): Int = a * b
  def divide(a: Int, b: Int): Int = a / b
  def remainder(a: Int, b: Int): Int = a % b

  def less(a: Int, b: Int): Boolean = a < b
  def lessOrEqual(a: Int, b: Int): Boolean = a <= b
  def greater(a: Int, b: Int): Boolean = a > b
  def greaterOrEqual(a: Int, b: Int): Boolean = a >= b
