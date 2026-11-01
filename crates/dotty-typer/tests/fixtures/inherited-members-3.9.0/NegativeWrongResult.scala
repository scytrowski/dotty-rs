package inheritedmembers

class Parent:
  def add(x: Int): Int = x

class Child extends Parent:
  def direct(x: Int): String = add(x)
