package inheritancetyper

class Parent:
  val bias: Int = 2
  def add(x: Int): Int = x + 1

class Child extends Parent:
  def direct(x: Int): Int = add(x)
  def withBias(x: Int): Int = x + bias
  def lifted: Int => Int = x => add(x)
  def mapped: Int => Int = x => x + bias
