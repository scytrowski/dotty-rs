class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current
  def doubled(): Int = value() + value()

class Left(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Right(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 2

class Runner:
  def evaluate(base: Base): Int = base.doubled()
  def fromLeft(seed: Int): Int = evaluate(new Left(seed))
  def fromRight(seed: Int): Int = evaluate(new Right(seed))
