package example
class BooleanLocals {
  def declared(x: Boolean): Boolean = { val flag: Boolean = x; flag }
  def inferred(x: Boolean): Boolean = { val flag = x; flag }
  def literal(): Boolean = { val flag = true; flag }
  def mutable(x: Boolean): Boolean = { var flag = false; flag = x; flag = !flag; flag }
  def conditional(x: Boolean): Boolean = { val flag = if x then false else true; flag }
  def shortInit(x: Boolean, y: Boolean): Boolean = { val flag = x && y; flag }
  def comparison(x: Int): Boolean = { val flag = x > 0; flag }
  def branchAssign(x: Boolean, y: Boolean): Boolean = { var flag = x; if y then flag = false else flag = true; flag }
  def loop(x: Boolean): Int = { var flag = x; var count = 0; while flag do { count = count + 1; flag = false }; count }
  def fromMatch(x: Boolean): Boolean = { val flag = x match { case true => false; case _ => true }; flag }
  def selector(x: Boolean): Int = { val flag = x; flag match { case true => 1; case _ => 2 } }
  def binder(x: Boolean): Boolean = x match { case n => { val flag = n; flag } }
  def effects(x: Boolean): Boolean = { var flag = false; val result = x && { flag = true; false }; flag || result }
  def guardEffects(x: Boolean, n: Int): Int = { var flag = x; n match { case 1 if { flag = !flag; flag } => 10; case _ => if flag then 20 else 30 } }
  def longResult(x: Boolean, n: Long): Long = { val flag = x; if flag then n else 0L }
  def doubleResult(x: Boolean, n: Double): Double = { val flag = x; if flag then n else 0.0 }
  def returnFlag(x: Boolean): Boolean = { val flag = x; return flag }
  def tailFormal(x: Boolean, y: Boolean): Boolean = if x then { return true; y } else false
  def tailLocal(x: Boolean, y: Boolean): Boolean = { val flag = y; if x then { return true; flag } else false }
  def tailPattern(x: Boolean): Boolean = x match { case n => { return true; n } }
  def floatResult(x: Boolean, n: Float): Float = { val flag = x; if flag then n else 0.0f }
  def assignMatch(x: Boolean): Boolean = { var flag = x; flag = x match { case true => false; case _ => true }; flag }
  def effectsOr(x: Boolean): Boolean = { var flag = false; val result = x || { flag = true; false }; flag && !result }
  def loopReturn(x: Boolean): Int = { var flag = x; while flag do { flag = false; return 1 }; 0 }
  def unit(x: Boolean): Unit = { var flag = x; flag = false; () }
  def tailSingleton(x: Boolean): Boolean = x match { case n @ true => { return false; n }; case _ => true }
  def tailConstant(): Boolean = true match { case n => { return false; n } }
  def siblingScopes(x: Boolean): Int = if x then { val flag = true; if flag then 1 else 2 } else { val flag = false; if flag then 3 else 4 }
  def parameterShadow(flag: Boolean): Boolean = { val flag = false; flag }
  def ascribed(x: Boolean): Boolean = { val flag = x; (flag: Boolean) }
  def bothReturn(x: Boolean, y: Boolean): Boolean = { val flag = y; if x then { return true; flag } else { return false; flag } }
}
