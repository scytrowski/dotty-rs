package example

class Counter(start: Int):
  private var current: Int = start

  def get(): Int = current
  def set(value: Int): Unit = current = value
