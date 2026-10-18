class Counter(start: Int):
  private var current: Int = start

  def get(): Int = current

  def increment(): Int =
    current = current + 1
    current

  def self(): Counter = this

class CounterService:
  def create(start: Int): Counter =
    new Counter(start)

  def passthrough(counter: Counter): Counter =
    counter

  def read(counter: Counter): Int =
    counter.get()

  def choose(flag: Boolean, a: Counter, b: Counter): Counter =
    if flag then a else b

  def incrementUntil(counter: Counter, limit: Int): Int =
    while counter.get() < limit do
      counter.increment()
    counter.get()

  def roundTrip(start: Int): Int =
    read(create(start))
