class Base:
  def value(): Int = 10
  def doubled(): Int =
    value() + value()

class Derived extends Base:
  override def value(): Int = 21

class Runner:
  def evaluate(base: Base): Int =
    base.doubled()

  def fromBase(): Int =
    evaluate(new Base())

  def fromDerived(): Int =
    evaluate(new Derived())

  def produceBase(): Base =
    new Derived()

  def branch(base: Base, flag: Boolean): Int =
    if flag then base.doubled() else base.value()
