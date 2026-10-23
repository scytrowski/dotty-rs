class Runner:
  def zero(): Long = 0L
  def one(): Long = 1L
  def constant(): Long = 9000000000L

  def identity(value: Long): Long = value

  def mixed(flag: Boolean, count: Int, value: Long, tail: Int): Long = value

  def roundTrip(value: Long): Long =
    val saved: Long = value
    saved

  def reassigned(value: Long, next: Long): Long =
    var saved: Long = value
    saved = next
    saved

  def explicit(value: Long): Long = return value
