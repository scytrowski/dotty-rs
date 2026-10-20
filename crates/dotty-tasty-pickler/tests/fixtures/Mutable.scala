package example
class Mutable:
  def overwrite: Int =
    var count: Int = 0
    count = 7
    count

  def increment(start: Int): Int =
    var count: Int = start
    count = count + 1
    count

  def sequential: Int =
    var count: Int = 0
    count = 7
    count = count + 1
    count

  def signed(start: Int): Int =
    var count: Int = start
    count = count + 1
    count

  def immutableTarget: Int =
    val fixed: Int = 0
    var count: Int = 1
    count = fixed + 2
    count
