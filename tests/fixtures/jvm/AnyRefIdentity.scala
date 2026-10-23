class Box(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Runner:
  def same(first: Box, second: Box): Boolean = first eq second
  def different(first: Box, second: Box): Boolean = first ne second
  def missing(value: Box): Boolean = value eq null
  def present(value: Box): Boolean = value ne null
  def choose(flag: Boolean, first: Box, second: Box): Boolean =
    if first eq second then flag else !flag
  def clearUntilNull(value: Box): Boolean =
    var current: Box = value
    while current ne null do current = null
    current eq null
