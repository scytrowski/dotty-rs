class Counter(start: Int):
  private var current: Int = start

  def get(): Int = current

  def increment(): Int =
    current = current + 1
    current

  def incrementUntil(limit: Int): Int =
    while current < limit do
      current = current + 1
    current

class CounterClient:
  def run(limit: Int): Int =
    val counter = new Counter(0)

    while counter.get() < limit do
      counter.increment()

    counter.get()

  def choose(flag: Boolean): Int =
    if flag then
      new Counter(10).get()
    else
      new Counter(20).get()
