package example

class Expressions {
  private var count: Int = 0
  def intValue(x: Int): Int = (x: Int)
  def booleanValue(x: Boolean): Boolean = (x: Boolean)
  def unitValue(): Unit = ((): Unit)
  def mask(x: Int): Int = x & 255
  def equal(x: Int, y: Int): Boolean = x == y
  def different(x: Int, y: Int): Boolean = x != y
  def classify(x: Int): Int = x match {
    case 0 => 10
    case 1 => 20
    case _ => 30
  }
  def flag(x: Int): Boolean = x match {
    case 0 => false
    case _ => true
  }
  def touch(x: Int): Unit = x match {
    case 0 => count = count + 1
    case _ => count = count + 2
  }
  def next(): Int = { count = count + 1; count }
  def read(): Int = count
  def once(): Int = next() match {
    case 1 => 11
    case _ => 22
  }
  def composed(raw: Int): Int = {
    val masked = (raw: Int) & 255
    masked match {
      case 0 => 10
      case 1 => if raw == 1 then 20 else 30
      case _ => 40
    }
  }
}
