package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Gauge(start: Int):
  private var reading: Int = start
  def get(): Int = reading
  def set(value: Int): Unit = reading = value
