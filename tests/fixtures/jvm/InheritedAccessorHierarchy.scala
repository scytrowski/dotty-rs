class Base(val id: Int):
  var current: Long = 0L
  val enabled: Boolean = true
  def doubled(): Int = id + id

class Middle(seed: Int) extends Base(seed):
  def read(): Long = current
  def write(value: Long): Unit = current = value

class Leaf(seed: Int) extends Middle(seed)

class Reader:
  def fromBase(base: Base): Long = base.current
  def fromMiddle(middle: Middle): Long = middle.current
  def fromLeaf(leaf: Leaf): Long = leaf.current
  def change(leaf: Leaf, value: Long): Unit = leaf.current = value
  def id(leaf: Leaf): Int = leaf.id
  def isEnabled(leaf: Leaf): Boolean = leaf.enabled
