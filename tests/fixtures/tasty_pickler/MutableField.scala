package example

class MutableField:
  private var current: Int = 0

  def get(): Int = current

  def set(value: Int): Unit =
    current = value

  def increment(): Int =
    current = current + 1
    current
