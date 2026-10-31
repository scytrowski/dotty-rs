package shadowfield
object Imported:
  val bias: Int = 4
class Parent:
  val bias: Int = 2
class Child extends Parent:
  def imported(x: Int): Int =
    import Imported.bias
    x + bias
