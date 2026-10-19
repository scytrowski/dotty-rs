package example

class CustomOperators:
  def +(right: Int): Int = right
  def use(right: Int): Int = this + right
