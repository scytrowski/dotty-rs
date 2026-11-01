package inheritedmembers

class Parent:
  val bias: Boolean = true

class Child extends Parent:
  def withBias(x: Int): Int = x + bias
