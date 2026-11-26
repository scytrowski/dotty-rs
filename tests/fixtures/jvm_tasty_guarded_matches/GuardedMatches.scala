package example
class GuardedMatches {
  private var trace: Int = 0
  def next(): Int = { trace = trace * 10 + 1; 5 }
  def guardCode(): Int = { trace = trace * 10 + 2; 0 }
  def observed(): Int = trace
  def run(): Int = next() match {
    case 0 if guardCode() > 0 => 99
    case n if guardCode() > 0 => n
    case n if n > 0 => n + 1
    case _ => 0
  }
  def bound(x: Int): Int = x match {
    case n @ 0 if n == 0 => n
    case n => n
  }
  def nested(x: Int): Int = x match {
    case n if n > 0 => n match {
      case m if m > 3 => m
      case _ => n
    }
    case _ => x
  }
  def constant(): Int = 0 match { case n => n }
  def negative(x: Int): Int = x match { case n @ -1 => n; case _ => 0 }
  def shadow(n: Int): Int = n match { case n if n > 0 => n + 1; case _ => n }
  def flag(x: Int, enabled: Boolean): Boolean = x match {
    case n if enabled && n > 0 => true
    case _ => false
  }
  def shortCircuit(enabled: Boolean): Int = 1 match {
    case _ if enabled && guardCode() > 0 => 99
    case _ => trace
  }
  def literalFirst(x: Int): Int = x match {
    case 0 if guardCode() > 0 => 99
    case 0 => 10
    case _ => 20
  }
  def touch(x: Int): Unit = x match { case n if n > 0 => trace = trace + n; case _ => () }
}
