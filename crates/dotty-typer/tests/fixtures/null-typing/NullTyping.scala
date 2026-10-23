class Box

class Runner:
  def exact(): Null = null
  def inferredNull = null
  def choose(flag: Boolean, other: Box): Box =
    if flag then null else other
  def chooseReverse(flag: Boolean, other: Box): Box =
    if flag then other else null
  def local(): Box =
    val box: Box = null
    box
  def badInt(): Int = null
  def badBoolean(): Boolean = null
