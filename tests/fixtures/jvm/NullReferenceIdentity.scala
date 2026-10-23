class Box(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Runner:
  def same(a: Box, b: Box): Boolean = a eq b
  def different(a: Box, b: Box): Boolean = a ne b
  def choose(flag: Boolean, value: Box): Box =
    if flag then null else value
  def readOrDefault(flag: Boolean, value: Box): Int =
    val result: Box = choose(flag, value)
    if result eq null then -1 else result.value()
