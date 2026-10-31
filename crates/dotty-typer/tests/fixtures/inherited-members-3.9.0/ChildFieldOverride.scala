package shadowfield
class Parent:
  val bias: Int = 2
class Child extends Parent:
  override val bias: Int = 3
  def own(x: Int): Int = x + bias
