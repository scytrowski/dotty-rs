package example
class Loop:
  def sumTo(n: Int): Int =
    var i: Int = 0
    var total: Int = 0
    while i < n do
      total = total + i
      i = i + 1
    total
