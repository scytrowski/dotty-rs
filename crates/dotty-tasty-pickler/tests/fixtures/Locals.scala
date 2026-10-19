package example

class Locals:
  def copy(value: Int): Int =
    val saved: Int = value
    saved
  def literal: Int =
    val saved: Int = 42
    saved
  def call(value: Int): Int =
    val saved: Int = identity(value)
    saved
  def multiple(value: Int): Int =
    val first: Int = value
    val second: Int = 7
    second
  def identity(value: Int): Int = value
