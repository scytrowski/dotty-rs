class Base(seed: Int):
  private val currentSeed: Int = seed
  def value(): Int = currentSeed

class Left(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 1

class Right(seed: Int) extends Base(seed):
  override def value(): Int = super.value() + 2

class Runner:
  def layout(seed: Int): Int =
    var current: Base = new Left(seed)
    42

  def shadowed(flag: Boolean, seed: Int): Int =
    var current: Base = new Left(seed)
    if flag then
      val current: Base = new Right(seed)
    42

  def choose(flag: Boolean, seed: Int): Int =
    var current: Base = new Left(seed)
    if flag then current = new Right(seed)
    42

  def repeat(n: Int, seed: Int): Int =
    var current: Base = new Left(seed)
    var i = 0
    while i < n do
      current = new Right(seed)
      i = i + 1
    42

  def initialize(flag: Boolean, seed: Int): Int =
    var current: Base =
      if flag then new Left(seed) else new Right(seed)
    42
