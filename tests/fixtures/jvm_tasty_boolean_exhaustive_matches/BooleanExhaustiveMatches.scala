package example
class BooleanExhaustiveMatches {
  def ordered(x: Boolean): Int = x match { case true => 1; case false => 2 }
  def reversed(x: Boolean): Int = x match { case false => 2; case true => 1 }
  def alternative(x: Boolean): Int = x match { case true | false => 3 }
  def nested_alternative(x: Boolean): Int = x match { case ((true | true) | false) => 3 }
  def bind_literals(x: Boolean): Boolean = x match { case n @ true => n; case n @ false => n }
  def guard_before_complete(x: Boolean, g: Boolean): Int = x match { case true if g => 3; case true => 1; case false => 2 }
  def guarded_alternative_before_complete(x: Boolean, g: Boolean): Int = x match { case true | true if g => 3; case true => 1; case false => 2 }
  def guard_effect(x: Boolean): Int = { var c = 0; val r = x match { case true | true if { c = c + 1; false } => 3; case true => 10; case false => 20 }; r + c }
  def selector_effect(x: Boolean): Int = { var c = 0; val r = ({ c = c + 1; x }) match { case true => 10; case false => 20 }; r + c }
  def return_mixed(x: Boolean): Int = x match { case true => return 1; case false => 2 }
  def return_all(x: Boolean): Int = x match { case true => return 1; case false => return 2 }
  def return_tail(x: Boolean, y: Boolean): Boolean = x match { case true => { return false; y }; case false => { return true; y } }
  def local_bool(x: Boolean): Boolean = { val b = x; b match { case true => false; case false => true } }
  def nested(x: Boolean,y: Boolean): Int = x match { case true => y match { case true => 1; case false => 2 }; case false => 3 }
  def result_boolean(x: Boolean): Boolean = x match { case true => true; case false => false }
  def return_boolean(x: Boolean, a: Boolean, b: Boolean): Boolean = x match { case true => return a; case false => b }
  def result_unit(x: Boolean): Unit = x match { case true => (); case false => () }
  def result_long(x: Boolean): Long = x match { case true => 9223372036854775807L; case false => -9223372036854775808L }
  def return_long(x: Boolean, a: Long, b: Long): Long = x match { case true => return a; case false => b }
  def result_float(x: Boolean): Float = x match { case true => -0.0f; case false => 1.4e-45f }
  def return_float(x: Boolean, a: Float, b: Float): Float = x match { case true => return a; case false => b }
  def result_double(x: Boolean): Double = x match { case true => -0.0; case false => 4.9e-324 }
  def return_double(x: Boolean, a: Double, b: Double): Double = x match { case true => return a; case false => b }
  def return_unit(x: Boolean): Unit = x match { case true => return (); case false => () }
  def orderedEffects(x: Boolean, g: Boolean): Int = { var trace = 0; val r = ({ trace = trace * 10 + 1; x }) match { case true | true if { trace = trace * 10 + 2; g } => { trace = trace * 10 + 3; 10 }; case true => { trace = trace * 10 + 4; 20 }; case false => { trace = trace * 10 + 5; 30 } }; trace * 100 + r }
  def assign(x: Boolean): Int = { var flag = x; flag = x match { case false => true; case true => false }; if flag then 1 else 0 }
  def shortCircuit(x: Boolean, y: Boolean): Int = { var n = 0; val b = x && (y match { case true => { n = n + 1; true }; case false => { n = n + 2; false } }); if b then n + 10 else n }
  def guardedWildcard(x: Boolean, g: Boolean): Int = x match { case _ if g => 5; case true => 1; case false => 2 }
  def matchLoop(x: Boolean): Int = { var flag = x; var count = 0; while flag do { flag = flag match { case true => false; case false => true }; count = count + 1 }; count }
  def intNested(x: Boolean, n: Int): Int = x match { case true => n match { case 1 => 10; case _ => 20 }; case false => 30 }
}
