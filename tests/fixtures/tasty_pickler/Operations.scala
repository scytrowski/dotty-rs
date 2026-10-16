package example
class Operations:
  def use: Int = identity(42)
  def identity(x: Int): Int = x
  def first: Int = second
  def second: Int = 7
  def zero(): Int = 0
