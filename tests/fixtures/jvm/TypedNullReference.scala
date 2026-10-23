class Box(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Runner:
  def accept(value: Box): Box = value
  def choose(flag: Boolean, value: Box): Box =
    if flag then null else value
  def chooseReverse(flag: Boolean, value: Box): Box =
    if flag then value else null
  def empty(): Box = null
  def emptyBlock(): Box = { 1; null }
  def bothNull(flag: Boolean): Box =
    if flag then null else null
  def passNullToFormal(): Box = accept(null)
  def passBlockNullToFormal(): Box = accept({ 1; null })
  def blockNullLocal(): Box =
    val result: Box = { 1; null }
    result
  def replace(flag: Boolean, value: Box): Box =
    var result: Box = null
    if flag then result = value
    result
  def clear(flag: Boolean, value: Box): Box =
    var result: Box = value
    if flag then result = null
    result
  def keepNull(flag: Boolean): Box =
    var result: Box = null
    if flag then 1 else 2
    result
