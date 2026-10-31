package inheritancetypernegative

class Parent:
  def add(x: Int): Int = x
class Child extends Parent:
  def wrong: Int = add(true)
