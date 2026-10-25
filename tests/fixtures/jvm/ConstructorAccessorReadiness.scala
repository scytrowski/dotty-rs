class Counter(val id: Int, var count: Long):
  def ownRead(): Long = this.count
  def ownWrite(next: Long): Unit =
    this.count = next
  def ownId(): Int = this.id
  def unqualifiedRead(): Long = count
  def unqualifiedWrite(next: Long): Unit = count = next

class Reader:
  def readId(c: Counter): Int = c.id
  def read(c: Counter): Long = c.count
  def write(c: Counter, next: Long): Unit =
    c.count = next
  def constructedId(): Int =
    val c = new Counter(1, 2L)
    c.id
  def constructedRead(): Long =
    val c = new Counter(1, 2L)
    c.count
  def constructedWrite(next: Long): Unit =
    val c = new Counter(1, 2L)
    c.count = next
  def unstableRead(): Long =
    var c = new Counter(1, 2L)
    c.count
  def unstableWrite(next: Long): Unit =
    var c = new Counter(1, 2L)
    c.count = next
