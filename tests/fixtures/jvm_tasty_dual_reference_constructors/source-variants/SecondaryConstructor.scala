package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Pair(first: Counter, second: Counter):
  private val left: Counter = first
  private val right: Counter = second
  def this(first: Counter, second: Counter, extra: Int) = this(first, second)
  def leftValue(): Int = left.get()
  def rightValue(): Int = right.get()
