class LongBox(initial: Long):
  private var current: Long = initial
  def get(): Long = current
  def set(value: Long): Unit =
    current = value

class LongConstant:
  private val number: Long = 9000000000L
  def get(): Long = number
