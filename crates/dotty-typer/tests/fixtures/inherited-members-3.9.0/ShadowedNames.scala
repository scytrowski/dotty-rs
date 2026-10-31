package inheritancetypernegative

class Parent:
  val bias: Int = 1
  def add(x: Int): Int = x + 1
class Child extends Parent:
  override def add(x: Int): Int = x + 2
  def member(x: Int): Int = add(x)
  def local(x: Int): Int =
    val bias = 3
    x + bias
