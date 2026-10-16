class Counter(start: Int):
  private var current: Int = start

  def increment(): Int =
    current = current + 1
    current

  def get(): Int = current

class CounterClient:
  def run(seed: Int): Int =
    val counter = new Counter(seed)
    counter.increment()
    counter.get()

  def direct(): Int =
    new Counter(42).get()

  def independent(): Int =
    val left = new Counter(10)
    val right = new Counter(20)
    left.increment()
    left.get() + right.get()
