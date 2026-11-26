package example
class AlternativeMatches {
  private var trace: Int = 0
  def next(x: Int): Int = { trace = trace * 10 + 1; x }
  def guardCode(): Int = { trace = trace * 10 + 2; 0 }
  def observed(): Int = trace
  def choose(x: Int): Int = x match { case -2147483648 | 0 | 2147483647 => 10; case _ => 20 }
  def guarded(x: Int): Int = next(x) match {
    case 0 | 0 if guardCode() > 0 => 10
    case 0 | 1 => 30
    case _ => 20
  }
  def returned(x: Int): Int = x match { case 0 | 1 => return 10; case _ => return 20 }
  def nested(x: Int): Int = x match { case (0 | 1) | 2 => 10; case _ => 20 }
  def flag(x: Int): Boolean = x match { case 0 | 1 => true; case _ => false }
  def unit(x: Int): Unit = x match { case 0 | 1 => (); case _ => () }
  def longValue(x: Int, a: Long, b: Long): Long = x match { case 0 | 1 => a; case _ => b }
  def longReturn(x: Int, a: Long, b: Long): Long = x match { case 0 | 1 => return a; case _ => return b }
  def guardedTrue(x: Int): Int = next(x) match { case 0 | 0 if guardCode() == 0 => 10; case _ => 20 }
  def bound(x: Int): Int = x match { case 0 | 1 => 10; case n if n < 0 => n; case _ => 20 }
  def local(x: Int): Int = { val saved: Int = x match { case 0 | 1 => 10; case _ => 20 }; saved }
  def mixed(x: Int, a: Long, b: Long): Long = x match { case 0 | 1 => return a; case _ => b }
  def inLoop(n: Int): Int = { var i: Int = n; while i > 0 do { i match { case 1 | 2 => return i; case _ => () }; i = i - 1 }; 0 }
  def nestedMatch(x: Int): Int = x match { case 0 | 1 => x match { case 0 | 1 => 10; case _ => 20 }; case _ => 30 }
}
