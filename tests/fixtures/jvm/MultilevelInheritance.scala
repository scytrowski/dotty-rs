class Base(seed: Int):
  private var current: Int = seed

  def value(): Int = current

  def doubled(): Int =
    value() + value()

class Middle(seed: Int) extends Base(seed):
  override def value(): Int =
    super.value() + 1

class Leaf(seed: Int) extends Middle(seed):
  override def value(): Int =
    super.value() + 1

  def parentValue(): Int =
    super.value()

class Runner:
  def evaluate(base: Base): Int =
    base.doubled()

  def run(seed: Int): Int =
    evaluate(new Leaf(seed))

  def choose(flag: Boolean, seed: Int): Middle =
    if flag then new Middle(seed)
    else new Leaf(seed)

  def reverseChoose(flag: Boolean, seed: Int): Middle =
    if flag then new Leaf(seed)
    else new Middle(seed)

  def chosenValue(flag: Boolean, seed: Int): Int =
    choose(flag, seed).value()

  def reverseChosenValue(flag: Boolean, seed: Int): Int =
    reverseChoose(flag, seed).value()

  def produceBase(seed: Int): Base =
    new Leaf(seed)
