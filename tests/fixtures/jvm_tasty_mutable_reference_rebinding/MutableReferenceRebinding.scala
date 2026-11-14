package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Pair(first: Counter, second: Counter):
  private val left: Counter = first
  private val right: Counter = second
  def leftValue(): Int = left.get()
  def rightValue(): Int = right.get()

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    var selected: Counter = first
    selected = second
    selected.set(nextValue)
    new Pair(first, second)
