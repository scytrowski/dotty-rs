class Counter(val id: Int, var count: Long) {
  def ownReadId: Int = this.id
  def ownReadCount: Long = this.count
  def ownReadUnqualified: Long = count
  def ownWrite(next: Long): Unit = this.count = next
}
class Reader {
  def readId(c: Counter): Int = c.id
  def readCount(c: Counter): Long = c.count
  def write(c: Counter, next: Long): Unit = c.count = next
  def readLocalVal: Int = { val c = new Counter(7, 9L); c.id }
  def readLocalCount: Long = { val c = new Counter(7, 9L); c.count }
  def writeLocalVal: Unit = { val c = new Counter(7, 9L); c.count = 11L }
}
class Other {}
