class Delegator:
  def identity(x: Int): Int = x
  def direct(x: Int): Int = identity(x)
  def explicit(x: Int): Int = this.identity(x)
  def chain(x: Int): Int = identity(identity(x))
  def constant(): Int = 42
  def zero(): Int = constant()
  def sequence(x: Int): Int =
    identity(x)
    identity(42)
