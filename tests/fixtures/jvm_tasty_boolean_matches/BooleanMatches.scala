package example
class BooleanMatches {
  private var trace: Int = 0
  def reset(): Unit = trace = 0
  def observed(): Int = trace
  def mark(n: Int): Int = { trace = trace * 10 + n; n }
  def select(x: Boolean): Int = (mark(1) > 0 && x) match {
    case true if mark(2) < 0 => 20
    case false if mark(3) > 0 => 30
    case _ => 40
  }
  def alternate(x: Boolean, flag: Boolean): Int = x match {
    case (true | true) | false if mark(2) > 0 && flag => 10
    case _ => 20
  }
  def bind(x: Boolean): Boolean = x match {
    case n @ true if n => n
    case n @ _ => n
  }
  def shadow(x: Boolean, n: Boolean): Boolean = x match {
    case n if n => n
    case n => n
  }
  def inverted(x: Boolean): Boolean = x match { case n => !n }
  def conjunction(x: Boolean, flag: Boolean): Boolean = x match { case n => n && flag }
  def falseSingleton(): Boolean = false match { case n => n }
  def loop(n: Int, x: Boolean): Int = { var i: Int = n; while i > 0 do { x match { case true => return i; case _ => () }; i = i - 1 }; 0 }
  def singleton(): Boolean = true match { case n => n }
  def nested(x: Boolean, y: Boolean): Boolean = x match {
    case n => y match { case m if m => n; case _ => n }
  }
  def nestedInt(x: Boolean, y: Int): Int = x match {
    case n if n => y match { case 0 => 7; case _ => 8 }
    case _ => 9
  }
  def longValue(x: Boolean, a: Long, b: Long): Long = x match { case true => a; case _ => b }
  def floatValue(x: Boolean, a: Float, b: Float): Float = x match { case true => a; case _ => b }
  def doubleValue(x: Boolean, a: Double, b: Double): Double = x match { case true => a; case _ => b }
  def returned(x: Boolean, a: Double, b: Double): Double = x match { case true => return a; case _ => return b }
  def mixed(x: Boolean): Int = x match { case true => return 1; case _ => 2 }
  def unitValue(x: Boolean): Unit = x match { case true => (); case _ => () }
  def shortCircuit(x: Boolean, y: Boolean): Int = (x && y) match { case true => 1; case _ => 2 }
}
