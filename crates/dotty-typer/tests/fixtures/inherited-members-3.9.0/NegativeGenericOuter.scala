package inheritedmembers

class Outer:
  def add(x: Int): Int = x
  class Parent:
    def add(x: Int): Int = x
    def add[A](x: A): Int = 0
  class Child extends Parent:
    def run(x: Int): Int = add(x)
