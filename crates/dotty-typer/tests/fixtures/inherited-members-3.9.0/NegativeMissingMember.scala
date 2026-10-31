package inheritancetypernegative

class Parent
class Child extends Parent:
  def missing(x: Int): Int = absent(x)
