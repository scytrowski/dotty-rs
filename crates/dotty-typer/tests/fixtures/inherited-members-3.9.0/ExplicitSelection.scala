package inheritancetyper

class ExplicitParent:
  val bias: Int = 2
  def add(x: Int): Int = x + 1

class ExplicitChild extends ExplicitParent:
  def direct(x: Int): Int = add(x)
  def explicitMethod(x: Int): Int = this.add(x)
  def withBias(x: Int): Int = x + bias
  def explicitField(x: Int): Int = x + this.bias
