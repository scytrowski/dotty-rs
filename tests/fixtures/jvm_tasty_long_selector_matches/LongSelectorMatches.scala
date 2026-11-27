package example
class LongSelectorMatches {
  def literal(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case 1L => a; case _ => z }
  def high_literal(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case 4294967297L => a; case _ => z }
  def extremes(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case -9223372036854775808L => a; case 9223372036854775807L => z; case _ => a }
  def alternative(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case 1L | 4294967297L => a; case _ => z }
  def nested_alternative(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case (1L | (4294967297L | -1L)) => a; case _ => z }
  def guard(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case 1L if b => a; case _ => z }
  def alternative_guard(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case 1L | 1L | 4294967297L if b => a; case _ => z }
  def binder(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n => n }
  def bound_literal(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n @ 1L => n; case n => n }
  def typed_binder(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n: Long => n }
  def typed_wildcard(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case _: Long => a }
  def explicit_typed_binder(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n @ (_: Long) => n }
  def guarded_binder(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n if b => n; case n => n }
  def guarded_typed(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n: Long if b => n; case _: Long => a }
  def nested(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n => n match { case 4294967297L => a; case k => k } }
  def typed_body(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n => (n: Long) }
  def returns(x: Long,b: Boolean,a: Long,z: Long): Long = x match { case n if b => return n; case n => return n }
  def result_Int_normal(x: Long,b: Boolean,a: Int): Int = x match {case 1L => a; case _ => 0}
  def result_Int_guarded(x: Long,b: Boolean,a: Int): Int = x match {case n: Long if b => a; case _: Long => 0}
  def result_Int_returns(x: Long,b: Boolean,a: Int): Int = x match {case 1L => return a; case _ => return 0}
  def result_Boolean_normal(x: Long,b: Boolean,a: Boolean): Boolean = x match {case 1L => a; case _ => false}
  def result_Boolean_guarded(x: Long,b: Boolean,a: Boolean): Boolean = x match {case n: Long if b => a; case _: Long => false}
  def result_Boolean_returns(x: Long,b: Boolean,a: Boolean): Boolean = x match {case 1L => return a; case _ => return false}
  def result_Unit_normal(x: Long,b: Boolean): Unit = x match {case 1L => (); case _ => ()}
  def result_Unit_guarded(x: Long,b: Boolean): Unit = x match {case n: Long if b => (); case _: Long => ()}
  def result_Unit_returns(x: Long,b: Boolean): Unit = x match {case 1L => return (); case _ => return ()}
  def result_Long_normal(x: Long,b: Boolean,a: Long): Long = x match {case 1L => a; case _ => 0L}
  def result_Long_guarded(x: Long,b: Boolean,a: Long): Long = x match {case n: Long if b => a; case _: Long => 0L}
  def result_Long_returns(x: Long,b: Boolean,a: Long): Long = x match {case 1L => return a; case _ => return 0L}
  def result_Float_normal(x: Long,b: Boolean,a: Float): Float = x match {case 1L => a; case _ => 0.0f}
  def result_Float_guarded(x: Long,b: Boolean,a: Float): Float = x match {case n: Long if b => a; case _: Long => 0.0f}
  def result_Float_returns(x: Long,b: Boolean,a: Float): Float = x match {case 1L => return a; case _ => return 0.0f}
  def result_Double_normal(x: Long,b: Boolean,a: Double): Double = x match {case 1L => a; case _ => 0.0}
  def result_Double_guarded(x: Long,b: Boolean,a: Double): Double = x match {case n: Long if b => a; case _: Long => 0.0}
  def result_Double_returns(x: Long,b: Boolean,a: Double): Double = x match {case 1L => return a; case _ => return 0.0}
  def selector_local(x: Long): Long = {val a: Long = x; a match {case n => n}}
  def selector_mutable(x: Long): Long = {var a: Long = 1L; a = x; a match {case n: Long => n}}
  def singleton(): Long = 4294967297L match {case n: Long => n}
  def singleton_literal(): Long = 4294967297L match {case 4294967297L => 7L; case _ => 8L}
  def ascribed_selector(x: Long): Long = (x: Long) match {case n => n}
  def local_result(x: Long): Long = { val a: Long = x match {case n => n}; a }
  def assignment_result(x: Long): Long = {var a: Long = 0L; a = x match {case n => n}; a}
  def effect(x: Long,b: Boolean): Int = {var trace = 0; val v = ({trace = trace * 10 + 1; x} match {case 1L | 1L | 4294967297L if {trace = trace * 10 + 2; b} => {trace = trace * 10 + 3; 7}; case n: Long => {trace = trace * 10 + 4; 8}}); trace * 100 + v}
  def shadow(n: Long,b: Boolean): Long = n match {case n: Long if b => n; case n: Long => n}
  def mixed_nested(x: Long,b: Boolean): Long = b match {case true => x match {case n: Long => n}; case false => x match {case 1L => 7L; case n => n}}
  def plain_singleton(): Long = 1L match {case n => n}
  def bound_singleton(): Long = 1L match {case n @ 1L => n; case n => n}
  def nested_singleton(): Long = 1L match {case n => n match {case k => k}}
  def inferred_local(x: Long): Long = {val a = x; a match {case n => n}}
  def while_selector(x: Long,b: Boolean): Long = {var a: Long = 0L; var run = b; while run do {a = x; run = false}; a match {case n => n}}
  def non_tail_return(x: Long,b: Boolean): Long = {x match {case n if b => return n; case _ => ()}; 0L}
}
