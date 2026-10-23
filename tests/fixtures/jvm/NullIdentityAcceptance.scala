class Box(seed: Int):
  private var current: Int = seed

  def value(): Int = current

class Runner:
  def same(a: Box, b: Box): Boolean =
    a eq b

  def different(a: Box, b: Box): Boolean =
    a ne b

  def missing(value: Box): Boolean =
    value eq null

  def choose(flag: Boolean, value: Box): Box =
    if flag then null else value

  def chooseReverse(flag: Boolean, value: Box): Box =
    if flag then value else null

  def empty(): Box =
    null

  def update(flag: Boolean, value: Box): Box =
    var current: Box = null
    if flag then current = value
    current

  def keepNull(flag: Boolean): Box =
    val current: Box = null
    if flag then null else current

  def readOrDefault(flag: Boolean, value: Box): Int =
    val result: Box = choose(flag, value)
    if result eq null then -1
    else result.value()
