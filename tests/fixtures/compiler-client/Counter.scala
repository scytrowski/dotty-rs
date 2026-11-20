package demo

class Counter:
  private val base: Int = 40
  private var current: Int = base + 2
  def value(): Int = current
  def add(delta: Int): Unit = current = current + delta
