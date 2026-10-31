package inheritancetypernegative

class Parent:
  private val secretField: Int = 1
  private def secretMethod(x: Int): Int = x
class Child extends Parent:
  def field: Int = secretField
  def method(x: Int): Int = secretMethod(x)
