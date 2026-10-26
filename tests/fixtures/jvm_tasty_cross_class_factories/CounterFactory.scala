package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class CounterFactory:
  def identity(counter: Counter): Counter = counter
  def create(seed: Int): Counter = new Counter(seed)
