class ChainBase(seed: Int):
  private var current: Int = seed

  def value(): Int = current

  def doubled(): Int = value() * 2

class ChainMiddle(seed: Int) extends ChainBase(seed):
  override def value(): Int = super.value() + 1

class ChainLeaf(seed: Int) extends ChainMiddle(seed):
  override def value(): Int = super.value() + 1

  def parentValue(): Int = super.value()

class ChainRunner:
  def throughBase(value: ChainBase): Int = value.doubled()
