class Base(seed: Int):
  private var current: Int = seed

  def value(): Int = current

  def increment(): Int =
    current = current + 1
    current

  def doubled(): Int =
    value() + value()

class Derived(start: Int) extends Base(start):
  override def value(): Int =
    super.value() + 1

  def original(): Int =
    super.value()

  def advance(): Int =
    super.increment()

class Runner:
  def evaluate(base: Base): Int =
    base.doubled()

  def fromBase(seed: Int): Int =
    evaluate(new Base(seed))

  def fromDerived(seed: Int): Int =
    evaluate(new Derived(seed))

  def original(seed: Int): Int =
    new Derived(seed).original()

  def advance(seed: Int): Int =
    new Derived(seed).advance()

  def afterAdvance(seed: Int): Int =
    val d = new Derived(seed)
    d.advance()
    d.value()
