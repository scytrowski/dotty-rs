package example
class FloatingSelectorMatches {
  def f_literal(x: Float, b: Boolean, a: Float): Int = x match {case 1.5f => 11; case _ => 33}
  def f_zero_order(x: Float, b: Boolean, a: Float): Int = x match {case 0.0f => 11; case -0.0f => 22; case _ => 33}
  def f_negative_zero_order(x: Float, b: Boolean, a: Float): Int = x match {case -0.0f => 11; case 0.0f => 22; case _ => 33}
  def f_extremes(x: Float, b: Boolean, a: Float): Int = x match {case 1.4e-45f => 11; case 3.4028235e38f => 22; case _ => 33}
  def f_alternatives(x: Float, b: Boolean, a: Float): Int = x match {case 0.0f | -0.0f | 1.5f if b => 11; case _ => 33}
  def f_nested_alternatives(x: Float, b: Boolean, a: Float): Int = x match {case 1.5f | (2.5f | 3.5f) => 11; case _ => 33}
  def f_binder(x: Float, b: Boolean, a: Float): Float = x match {case n => n}
  def f_bound_literal(x: Float, b: Boolean, a: Float): Float = x match {case n @ 1.5f => n; case n => n}
  def f_typed_binder(x: Float, b: Boolean, a: Float): Float = x match {case n: Float => n}
  def f_typed_wildcard(x: Float, b: Boolean, a: Float): Int = x match {case _: Float => 42}
  def f_explicit_typed_binder(x: Float, b: Boolean, a: Float): Float = x match {case n @ (_: Float) => n}
  def f_guarded_binder(x: Float, b: Boolean, a: Float): Float = x match {case n if b => n; case _ => a}
  def f_guarded_typed(x: Float, b: Boolean, a: Float): Float = x match {case n: Float if b => n; case _: Float => a}
  def f_nested(x: Float, b: Boolean, a: Float): Float = x match {case n => n match {case 1.5f => a; case _: Float => n}}
  def f_ascribed_selector(x: Float, b: Boolean, a: Float): Float = (x: Float) match {case n => n}
  def f_explicit_local(x: Float, b: Boolean, a: Float): Float = {val saved: Float = x; saved match {case n => n}}
  def f_inferred_local(x: Float, b: Boolean, a: Float): Float = {val saved = x; saved match {case n => n}}
  def f_mutable_selector(x: Float, b: Boolean, a: Float): Float = {var saved: Float = x; saved = if b then x else 0.0f; saved match {case n => n}}
  def f_local_result(x: Float, b: Boolean, a: Float): Float = {val saved: Float = x match {case n => n}; saved}
  def f_assignment_result(x: Float, b: Boolean, a: Float): Float = {var saved: Float = x; saved = x match {case 1.5f => a; case n => n}; saved}
  def f_effect(x: Float, b: Boolean, a: Float): Int = {var steps: Int = 0; val picked: Int = ({steps = steps * 10 + 1; x}) match {case 0.0f | -0.0f | 1.5f if {steps = steps * 10 + 2; b} => {steps = steps * 10 + 3; 7}; case _ => {steps = steps * 10 + 4; 8}}; steps * 100 + picked}
  def f_shadow(x: Float, b: Boolean, a: Float): Float = {val n: Float = x; x match {case n if b => n; case _: Float => n}}
  def f_mixed_nested(x: Float, b: Boolean, a: Float): Float = b match {case true => x match {case n => n}; case false => x match {case 1.5f => a; case n: Float => n}}
  def f_while_selector(x: Float, b: Boolean, a: Float): Float = ({var i: Int = 1; var saved: Float = x; while i > 0 do {saved = if b then x else 0.0f; i = i - 1}; saved}) match {case n => n}
  def f_all_returns(x: Float, b: Boolean, a: Float): Float = x match {case 1.5f => return a; case n => return n}
  def f_non_tail_return(x: Float, b: Boolean, a: Float): Float = {if b then return x; x match {case 1.5f => a; case n => n}}
  def f_plain_singleton(x: Float, b: Boolean, a: Float): Float = -0.0f match {case n => n}
  def f_bound_singleton(x: Float, b: Boolean, a: Float): Float = 1.5f match {case n @ 1.5f => n; case n => n}
  def f_typed_singleton(x: Float, b: Boolean, a: Float): Float = -0.0f match {case n: Float => n}
  def f_nested_singleton(x: Float, b: Boolean, a: Float): Float = 1.5f match {case n => n match {case k => k}}
  def f_result_Int_normal(x: Float, b: Boolean, a: Float): Int = x match {case 1.5f => 7; case _ => 0}
  def f_result_Int_guarded(x: Float, b: Boolean, a: Float): Int = x match {case n: Float if b => 7; case _: Float => 0}
  def f_result_Int_returns(x: Float, b: Boolean, a: Float): Int = x match {case 1.5f => return 7; case _ => return 0}
  def f_result_Boolean_normal(x: Float, b: Boolean, a: Float): Boolean = x match {case 1.5f => true; case _ => false}
  def f_result_Boolean_guarded(x: Float, b: Boolean, a: Float): Boolean = x match {case n: Float if b => true; case _: Float => false}
  def f_result_Boolean_returns(x: Float, b: Boolean, a: Float): Boolean = x match {case 1.5f => return true; case _ => return false}
  def f_result_Unit_normal(x: Float, b: Boolean, a: Float): Unit = x match {case 1.5f => (); case _ => ()}
  def f_result_Unit_guarded(x: Float, b: Boolean, a: Float): Unit = x match {case n: Float if b => (); case _: Float => ()}
  def f_result_Unit_returns(x: Float, b: Boolean, a: Float): Unit = x match {case 1.5f => return (); case _ => return ()}
  def f_result_Long_normal(x: Float, b: Boolean, a: Float): Long = x match {case 1.5f => 9223372036854775807L; case _ => 0L}
  def f_result_Long_guarded(x: Float, b: Boolean, a: Float): Long = x match {case n: Float if b => 9223372036854775807L; case _: Float => 0L}
  def f_result_Long_returns(x: Float, b: Boolean, a: Float): Long = x match {case 1.5f => return 9223372036854775807L; case _ => return 0L}
  def f_result_Float_normal(x: Float, b: Boolean, a: Float): Float = x match {case 1.5f => -0.0f; case _ => 0.0f}
  def f_result_Float_guarded(x: Float, b: Boolean, a: Float): Float = x match {case n: Float if b => -0.0f; case _: Float => 0.0f}
  def f_result_Float_returns(x: Float, b: Boolean, a: Float): Float = x match {case 1.5f => return -0.0f; case _ => return 0.0f}
  def f_result_Double_normal(x: Float, b: Boolean, a: Float): Double = x match {case 1.5f => -0.0d; case _ => 0.0d}
  def f_result_Double_guarded(x: Float, b: Boolean, a: Float): Double = x match {case n: Float if b => -0.0d; case _: Float => 0.0d}
  def f_result_Double_returns(x: Float, b: Boolean, a: Float): Double = x match {case 1.5f => return -0.0d; case _ => return 0.0d}
  def d_literal(x: Double, b: Boolean, a: Double): Int = x match {case 1.5d => 11; case _ => 33}
  def d_zero_order(x: Double, b: Boolean, a: Double): Int = x match {case 0.0d => 11; case -0.0d => 22; case _ => 33}
  def d_negative_zero_order(x: Double, b: Boolean, a: Double): Int = x match {case -0.0d => 11; case 0.0d => 22; case _ => 33}
  def d_extremes(x: Double, b: Boolean, a: Double): Int = x match {case 4.9e-324d => 11; case 1.7976931348623157e308d => 22; case _ => 33}
  def d_alternatives(x: Double, b: Boolean, a: Double): Int = x match {case 0.0d | -0.0d | 1.5d if b => 11; case _ => 33}
  def d_nested_alternatives(x: Double, b: Boolean, a: Double): Int = x match {case 1.5d | (2.5d | 3.5d) => 11; case _ => 33}
  def d_binder(x: Double, b: Boolean, a: Double): Double = x match {case n => n}
  def d_bound_literal(x: Double, b: Boolean, a: Double): Double = x match {case n @ 1.5d => n; case n => n}
  def d_typed_binder(x: Double, b: Boolean, a: Double): Double = x match {case n: Double => n}
  def d_typed_wildcard(x: Double, b: Boolean, a: Double): Int = x match {case _: Double => 42}
  def d_explicit_typed_binder(x: Double, b: Boolean, a: Double): Double = x match {case n @ (_: Double) => n}
  def d_guarded_binder(x: Double, b: Boolean, a: Double): Double = x match {case n if b => n; case _ => a}
  def d_guarded_typed(x: Double, b: Boolean, a: Double): Double = x match {case n: Double if b => n; case _: Double => a}
  def d_nested(x: Double, b: Boolean, a: Double): Double = x match {case n => n match {case 1.5d => a; case _: Double => n}}
  def d_ascribed_selector(x: Double, b: Boolean, a: Double): Double = (x: Double) match {case n => n}
  def d_explicit_local(x: Double, b: Boolean, a: Double): Double = {val saved: Double = x; saved match {case n => n}}
  def d_inferred_local(x: Double, b: Boolean, a: Double): Double = {val saved = x; saved match {case n => n}}
  def d_mutable_selector(x: Double, b: Boolean, a: Double): Double = {var saved: Double = x; saved = if b then x else 0.0d; saved match {case n => n}}
  def d_local_result(x: Double, b: Boolean, a: Double): Double = {val saved: Double = x match {case n => n}; saved}
  def d_assignment_result(x: Double, b: Boolean, a: Double): Double = {var saved: Double = x; saved = x match {case 1.5d => a; case n => n}; saved}
  def d_effect(x: Double, b: Boolean, a: Double): Int = {var steps: Int = 0; val picked: Int = ({steps = steps * 10 + 1; x}) match {case 0.0d | -0.0d | 1.5d if {steps = steps * 10 + 2; b} => {steps = steps * 10 + 3; 7}; case _ => {steps = steps * 10 + 4; 8}}; steps * 100 + picked}
  def d_shadow(x: Double, b: Boolean, a: Double): Double = {val n: Double = x; x match {case n if b => n; case _: Double => n}}
  def d_mixed_nested(x: Double, b: Boolean, a: Double): Double = b match {case true => x match {case n => n}; case false => x match {case 1.5d => a; case n: Double => n}}
  def d_while_selector(x: Double, b: Boolean, a: Double): Double = ({var i: Int = 1; var saved: Double = x; while i > 0 do {saved = if b then x else 0.0d; i = i - 1}; saved}) match {case n => n}
  def d_all_returns(x: Double, b: Boolean, a: Double): Double = x match {case 1.5d => return a; case n => return n}
  def d_non_tail_return(x: Double, b: Boolean, a: Double): Double = {if b then return x; x match {case 1.5d => a; case n => n}}
  def d_plain_singleton(x: Double, b: Boolean, a: Double): Double = -0.0d match {case n => n}
  def d_bound_singleton(x: Double, b: Boolean, a: Double): Double = 1.5d match {case n @ 1.5d => n; case n => n}
  def d_typed_singleton(x: Double, b: Boolean, a: Double): Double = -0.0d match {case n: Double => n}
  def d_nested_singleton(x: Double, b: Boolean, a: Double): Double = 1.5d match {case n => n match {case k => k}}
  def d_raw_pattern(x: Double, b: Boolean, a: Double): Int = x match {case 1.5 => 11; case _ => 33}
  def d_result_Int_normal(x: Double, b: Boolean, a: Double): Int = x match {case 1.5d => 7; case _ => 0}
  def d_result_Int_guarded(x: Double, b: Boolean, a: Double): Int = x match {case n: Double if b => 7; case _: Double => 0}
  def d_result_Int_returns(x: Double, b: Boolean, a: Double): Int = x match {case 1.5d => return 7; case _ => return 0}
  def d_result_Boolean_normal(x: Double, b: Boolean, a: Double): Boolean = x match {case 1.5d => true; case _ => false}
  def d_result_Boolean_guarded(x: Double, b: Boolean, a: Double): Boolean = x match {case n: Double if b => true; case _: Double => false}
  def d_result_Boolean_returns(x: Double, b: Boolean, a: Double): Boolean = x match {case 1.5d => return true; case _ => return false}
  def d_result_Unit_normal(x: Double, b: Boolean, a: Double): Unit = x match {case 1.5d => (); case _ => ()}
  def d_result_Unit_guarded(x: Double, b: Boolean, a: Double): Unit = x match {case n: Double if b => (); case _: Double => ()}
  def d_result_Unit_returns(x: Double, b: Boolean, a: Double): Unit = x match {case 1.5d => return (); case _ => return ()}
  def d_result_Long_normal(x: Double, b: Boolean, a: Double): Long = x match {case 1.5d => 9223372036854775807L; case _ => 0L}
  def d_result_Long_guarded(x: Double, b: Boolean, a: Double): Long = x match {case n: Double if b => 9223372036854775807L; case _: Double => 0L}
  def d_result_Long_returns(x: Double, b: Boolean, a: Double): Long = x match {case 1.5d => return 9223372036854775807L; case _ => return 0L}
  def d_result_Float_normal(x: Double, b: Boolean, a: Double): Float = x match {case 1.5d => -0.0f; case _ => 0.0f}
  def d_result_Float_guarded(x: Double, b: Boolean, a: Double): Float = x match {case n: Double if b => -0.0f; case _: Double => 0.0f}
  def d_result_Float_returns(x: Double, b: Boolean, a: Double): Float = x match {case 1.5d => return -0.0f; case _ => return 0.0f}
  def d_result_Double_normal(x: Double, b: Boolean, a: Double): Double = x match {case 1.5d => -0.0d; case _ => 0.0d}
  def d_result_Double_guarded(x: Double, b: Boolean, a: Double): Double = x match {case n: Double if b => -0.0d; case _: Double => 0.0d}
  def d_result_Double_returns(x: Double, b: Boolean, a: Double): Double = x match {case 1.5d => return -0.0d; case _ => return 0.0d}
  def f_bind_positive_zero(x: Float, b: Boolean, a: Float): Float = x match {case n @ 0.0f => n; case n => n}
  def f_bind_negative_zero(x: Float, b: Boolean, a: Float): Float = x match {case n @ -0.0f => n; case n => n}
  def f_singleton_bind_negative_zero(x: Float, b: Boolean, a: Float): Float = -0.0f match {case n @ -0.0f => n; case n => n}
  def d_bind_positive_zero(x: Double, b: Boolean, a: Double): Double = x match {case n @ 0.0d => n; case n => n}
  def d_bind_negative_zero(x: Double, b: Boolean, a: Double): Double = x match {case n @ -0.0d => n; case n => n}
  def d_singleton_bind_negative_zero(x: Double, b: Boolean, a: Double): Double = -0.0d match {case n @ -0.0d => n; case n => n}
}
