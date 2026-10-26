package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class CounterOwner(seed: Int):
  private val inner: Counter = new Counter(seed)
  def read(): Int = inner.get()
