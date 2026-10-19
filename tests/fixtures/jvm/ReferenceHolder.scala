class Counter(start: Int):
  private var count: Int = start

  def get(): Int = count

  def increment(): Int =
    count = count + 1
    count

class Holder(initial: Counter, enabled: Boolean):
  private val original: Counter = initial
  private var current: Counter = initial
  private val enabledFlag: Boolean = enabled

  def value(): Counter = current
  def initial(): Counter = original
  def replace(next: Counter): Unit =
    current = next

  def read(): Int = value().get()
  def isEnabled(): Boolean = enabledFlag
  def choose(flag: Boolean, alternate: Counter): Counter =
    if flag then value() else alternate

  def incrementUntil(limit: Int): Int =
    while value().get() < limit do
      value().increment()
    value().get()

class HolderClient:
  def wrap(counter: Counter): Holder =
    new Holder(counter, true)

  def run(start: Int, limit: Int): Int =
    val counter = new Counter(start)
    val holder = new Holder(counter, true)
    holder.incrementUntil(limit)
