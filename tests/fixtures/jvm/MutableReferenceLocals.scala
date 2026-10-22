class Base(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Left(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Right(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 2

class Runner:
  def choose(flag: Boolean, seed: Int): Int =
    var current: Base = new Left(seed)
    if flag then current = new Right(seed)
    current.value()

  def repeat(n: Int, seed: Int): Int =
    var current: Base = new Left(seed)
    var i = 0
    while i < n do
      current = new Right(seed)
      i = i + 1
    current.value()

  def initialize(flag: Boolean, seed: Int): Int =
    var current: Base =
      if flag then new Left(seed) else new Right(seed)
    current.value()

  def swap(flag: Boolean, seed: Int): Int =
    var current: Base = new Left(seed)
    current = if flag then new Right(seed) else new Left(seed)
    current.value()
