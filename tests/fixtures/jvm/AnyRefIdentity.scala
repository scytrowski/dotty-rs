class Box(seed: Int):
  private var current: Int = seed
  def value(): Int = current

class Runner:
  def same(first: Box, second: Box): Boolean = first eq second
  def different(first: Box, second: Box): Boolean = first ne second
  def missing(value: Box): Boolean = value eq null
  def present(value: Box): Boolean = value ne null
  def blockMissing(value: Box): Boolean = value eq { val marker = 1; null }
  def bothMissing(value: Box, flag: Boolean): Boolean = value eq (if flag then null else null)
  def maybeMissing(value: Box, other: Box, flag: Boolean): Boolean = value eq (if flag then null else other)
  def maybeReceiver(flag: Boolean, value: Box): Boolean = (if flag then null else value) eq value
  def choose(flag: Boolean, first: Box, second: Box): Boolean =
    if first eq second then flag else !flag
  def clearUntilNull(value: Box): Boolean =
    var current: Box = value
    while current ne null do current = null
    current eq null
