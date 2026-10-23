class LongBox(initial: Long):
  private var current: Long = initial
  def get(): Long = current
  def set(value: Long): Unit =
    current = value

class Runner:
  def constant(): Long = 9000000000L
  def roundTrip(value: Long): Long =
    val saved: Long = value
    saved
  def choose(flag: Boolean, left: Long, right: Long): Long =
    if flag then left else right
  def repeat(n: Int, first: Long, second: Long): Long =
    var saved: Long = first
    var i = 0
    while i < n do
      saved = second
      i = i + 1
    saved
