class Base {
  def value(): Int = 20
  def doubled(): Int = value() * 2
}

class Derived extends Base {
  override def value(): Int = 21
}

class Runner {
  def evaluate(base: Base): Int = base.doubled()
  def fromBase(): Int = evaluate(new Base())
  def fromDerived(): Int = evaluate(new Derived())
}
