package example
class Loop:
  def sumTo(n: Int): Int =
    var i: Int = 0
    var total: Int = 0
    while i < n do
      total = total + i
      i = i + 1
    total
  def run(flag: Boolean): Int =
    var count: Int = 0
    while flag do count = count + 1
    count
  def emptyBody(): Int =
    while false do ()
    0
  def assignmentThenUnit(): Int =
    var count: Int = 0
    while false do
      count = count + 1
      ()
    count
