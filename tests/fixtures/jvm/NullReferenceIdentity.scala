class Box(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Runner:
  def same(a: Box, b: Box): Boolean = a eq b
  def selfSame(a: Box): Boolean = a eq a
  def sameLocal(a: Box, b: Box): Boolean =
    val receiver: Box = a
    receiver eq b
  def different(a: Box, b: Box): Boolean = a ne b
  def isMissing(a: Box): Boolean = a eq null
  def choose(flag: Boolean, value: Box): Box =
    if flag then null else value
  def readOrDefault(flag: Boolean, value: Box): Int =
    val result: Box = choose(flag, value)
    if result eq null then -1 else result.value()
