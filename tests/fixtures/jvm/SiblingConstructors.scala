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
  def choose(flag: Boolean, seed: Int): Base =
    if flag then new Left(seed) else new Right(seed)
  def reverseChoose(flag: Boolean, seed: Int): Base =
    if flag then new Right(seed) else new Left(seed)
  def run(flag: Boolean, seed: Int): Int = choose(flag, seed).doubled()
  def acceptChoice(flag: Boolean, seed: Int): Int =
    evaluate(if flag then new Left(seed) else new Right(seed))
