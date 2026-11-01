package inheritedcaptureoracle

class Parent:
  val bias: Int = 2
  def add(x: Int): Int = x + 1

class Child extends Parent:
  def lifted: Int => Int = x => add(x)
  def mapped: Int => Int = x => x + bias
  def explicitMethod: Int => Int = x => this.add(x)
  def explicitField: Int => Int = x => x + this.bias
  def repeated: Int => Int = x => add(x) + bias + add(x) + bias
