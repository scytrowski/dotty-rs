class Parent:
  val id: Long = 42L
  var current: Int = 1

class Child extends Parent:
  def read(): Int = current
  def readThis(): Int = this.current
  def write(value: Int): Unit = current = value
  def writeThis(value: Int): Unit = this.current = value
  def readChild(child: Child): Int = child.current
  def readParent(parent: Parent): Int = parent.current
  def readChildAsParent(child: Child): Int = readParent(child)

class Leaf extends Child

class AccessorParent(val token: Int, var count: Long)
class AccessorChild extends AccessorParent(7, 9L):
  def readToken(): Int = token
  def readCount(): Long = count
  def writeCount(value: Long): Unit = count = value

class Reader:
  def readChild(child: Child): Int = child.current
  def readLeaf(leaf: Leaf): Int = leaf.current
  def readParent(parent: Parent): Int = parent.current
  def readToken(child: AccessorChild): Int = child.token
  def readCount(child: AccessorChild): Long = child.count
  def writeCount(child: AccessorChild, value: Long): Unit = child.count = value
