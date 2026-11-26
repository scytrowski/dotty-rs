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
}
