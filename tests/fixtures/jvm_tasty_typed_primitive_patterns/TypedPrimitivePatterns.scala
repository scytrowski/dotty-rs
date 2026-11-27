package example
class TypedPrimitivePatterns {
  def int_wildcard(x: Int, b: Boolean): Int = x match {case _: Int => 1}
  def int_binder(x: Int, b: Boolean): Int = x match {case n: Int => n}
  def int_explicit_binder(x: Int, b: Boolean): Int = x match {case n @ (_: Int) => n}
  def int_literal_prefix(x: Int, b: Boolean): Int = x match {case 1 => 1; case n: Int => n}
  def int_guarded_binder(x: Int, b: Boolean): Int = x match {case n: Int if b => n; case _ => 0}
  def int_guarded_wildcard(x: Int, b: Boolean): Int = x match {case _: Int if b => 1; case _ => 0}
  def int_two_binders(x: Int, b: Boolean): Int = x match {case n: Int if b => n; case n: Int => n}
  def int_nested(x: Int, b: Boolean): Int = x match {case n: Int => n match {case k: Int => k}}
  def int_typed_body(x: Int, b: Boolean): Int = x match {case n: Int => (n: Int)}
  def int_local_value(x: Int, b: Boolean): Int = {val a: Int = x match {case n: Int => n}; a}
  def int_assignment(x: Int, b: Boolean): Int = {var a: Int = 0; a = x match {case n: Int => n}; a}
  def int_returns(x: Int, b: Boolean): Int = x match {case n: Int if b => return n; case n: Int => return n}
  def int_result_int(x: Int, b: Boolean): Int = x match {case _: Int => 7}
  def int_result_boolean(x: Int, b: Boolean): Boolean = x match {case _: Int => b}
  def int_result_unit(x: Int, b: Boolean): Unit = x match {case _: Int => ()}
  def int_result_long(x: Int, b: Boolean, a: Long): Long = x match {case _: Int => a}
  def int_result_float(x: Int, b: Boolean, a: Float): Float = x match {case _: Int => a}
  def int_result_double(x: Int, b: Boolean, a: Double): Double = x match {case _: Int => a}
  def boolean_wildcard(x: Boolean, b: Boolean): Boolean = x match {case _: Boolean => true}
  def boolean_binder(x: Boolean, b: Boolean): Boolean = x match {case n: Boolean => n}
  def boolean_explicit_binder(x: Boolean, b: Boolean): Boolean = x match {case n @ (_: Boolean) => n}
  def boolean_literal_prefix(x: Boolean, b: Boolean): Boolean = x match {case true => true; case n: Boolean => n}
  def boolean_guarded_binder(x: Boolean, b: Boolean): Boolean = x match {case n: Boolean if b => n; case _ => false}
  def boolean_guarded_wildcard(x: Boolean, b: Boolean): Boolean = x match {case _: Boolean if b => true; case _ => false}
  def boolean_two_binders(x: Boolean, b: Boolean): Boolean = x match {case n: Boolean if b => n; case n: Boolean => n}
  def boolean_nested(x: Boolean, b: Boolean): Boolean = x match {case n: Boolean => n match {case k: Boolean => k}}
  def boolean_typed_body(x: Boolean, b: Boolean): Boolean = x match {case n: Boolean => (n: Boolean)}
  def boolean_local_value(x: Boolean, b: Boolean): Boolean = {val a: Boolean = x match {case n: Boolean => n}; a}
  def boolean_assignment(x: Boolean, b: Boolean): Boolean = {var a: Boolean = false; a = x match {case n: Boolean => n}; a}
  def boolean_returns(x: Boolean, b: Boolean): Boolean = x match {case n: Boolean if b => return n; case n: Boolean => return n}
  def boolean_result_int(x: Boolean, b: Boolean): Int = x match {case _: Boolean => 7}
  def boolean_result_boolean(x: Boolean, b: Boolean): Boolean = x match {case _: Boolean => b}
  def boolean_result_unit(x: Boolean, b: Boolean): Unit = x match {case _: Boolean => ()}
  def boolean_result_long(x: Boolean, b: Boolean, a: Long): Long = x match {case _: Boolean => a}
  def boolean_result_float(x: Boolean, b: Boolean, a: Float): Float = x match {case _: Boolean => a}
  def boolean_result_double(x: Boolean, b: Boolean, a: Double): Double = x match {case _: Boolean => a}
  def selector_effect(x: Int,b: Boolean): Int = { var trace = 0; val value = ({ trace = trace * 10 + 1; x } match { case n: Int if { trace = trace * 10 + 2; b } => { trace = trace * 10 + 3; n }; case m: Int => { trace = trace * 10 + 4; m } }); trace * 100 + value }
  def guard_skipped(x: Int,b: Boolean): Int = { var trace = 0; val value = ({ trace = trace * 10 + 1; x } match { case 0 if { trace = trace * 10 + 2; b } => { trace = trace * 10 + 3; 0 }; case n: Int => { trace = trace * 10 + 4; n } }); trace * 100 + value }
  def boolean_effect(x: Boolean,b: Boolean): Int = { var trace = 0; val value = ({ trace = trace * 10 + 1; x } match { case n: Boolean if { trace = trace * 10 + 2; b } => { trace = trace * 10 + 3; if n then 10 else 20 }; case m: Boolean => { trace = trace * 10 + 4; if m then 10 else 20 } }); trace * 100 + value }
  def shadow(n: Int,b: Boolean): Int = n match { case n: Int if b => n; case n: Int => n }
  def singleton_int(): Int = 1 match {case n: Int => n}
  def singleton_boolean(): Boolean = true match {case n: Boolean => n}
  def parenthesized(x: Int): Int = x match {case (_: Int) => 1}
  def binder_guard_int(x: Int): Int = x match {case n: Int if n > 0 => n; case _: Int => 0}
  def binder_guard_boolean(x: Boolean): Int = x match {case n: Boolean if n => 1; case _: Boolean => 0}
}
