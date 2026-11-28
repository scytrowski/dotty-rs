package example
class PrimitiveValueDiscard {
  def int_formal(x: Int, y: Int, b: Boolean): Unit = x
  def int_literal(x: Int, y: Int, b: Boolean): Unit = 1
  def int_ascription(x: Int, y: Int, b: Boolean): Unit = (x: Int)
  def int_if_unit(x: Int, y: Int, b: Boolean): Unit = if b then x else y
  def int_match_unit(x: Int, y: Int, b: Boolean): Unit = b match {case true => x;case false => y}
  def int_return_value(x: Int, y: Int, b: Boolean): Unit = return x
  def int_block_unit(x: Int, y: Int, b: Boolean): Unit = {var n: Int = 0; n = n + 1; x}
  def int_mixed_unit(x: Int, y: Int, b: Boolean): Unit = {if b then return (); x}
  def int_nested_unit(x: Int, y: Int, b: Boolean): Unit = {{x; 1}; 1}
  def int_statement(x: Int, y: Int, b: Boolean): Int = {x; 7}
  def int_local_read(x: Int, y: Int, b: Boolean): Int = {val z: Int = x; z; 7}
  def int_if_no_else(x: Int, y: Int, b: Boolean): Int = {var steps: Int = 0; if b then {steps = steps + 1; x}; steps}
  def int_while_body(x: Int, y: Int, b: Boolean): Int = {var steps: Int = 0; while steps < 2 do {steps = steps + 1; x}; steps}
  def int_effect_order(x: Int, y: Int, b: Boolean): Int = {var steps: Int = 0; {steps = steps * 10 + 1; x}; {steps = steps * 10 + 2; y}; steps}
  def int_match_statement(x: Int, y: Int, b: Boolean): Int = {var steps: Int = 0; b match {case true => {steps = 11; x}; case false => {steps = 22; y}}; steps}
  def int_preserve_result(x: Int, y: Int, b: Boolean): Int = {x; y}
  def int_selector_statement(x: Int, y: Int, b: Boolean): Int = {x; b} match {case true => 1;case false => 2}
  def int_guard_statement(x: Int, y: Int, b: Boolean): Int = b match {case n if {x; n} => 1;case _ => 2}
  def int_mixed_return(x: Int, y: Int, b: Boolean): Int = {if b then return 3; x; 7}
  def int_all_returning(x: Int, y: Int, b: Boolean): Int = {if b then return 3 else return 4; x; 7}
  def boolean_formal(x: Boolean, y: Boolean, b: Boolean): Unit = x
  def boolean_literal(x: Boolean, y: Boolean, b: Boolean): Unit = true
  def boolean_ascription(x: Boolean, y: Boolean, b: Boolean): Unit = (x: Boolean)
  def boolean_if_unit(x: Boolean, y: Boolean, b: Boolean): Unit = if b then x else y
  def boolean_match_unit(x: Boolean, y: Boolean, b: Boolean): Unit = b match {case true => x;case false => y}
  def boolean_return_value(x: Boolean, y: Boolean, b: Boolean): Unit = return x
  def boolean_block_unit(x: Boolean, y: Boolean, b: Boolean): Unit = {var n: Int = 0; n = n + 1; x}
  def boolean_mixed_unit(x: Boolean, y: Boolean, b: Boolean): Unit = {if b then return (); x}
  def boolean_nested_unit(x: Boolean, y: Boolean, b: Boolean): Unit = {{x; 1}; true}
  def boolean_statement(x: Boolean, y: Boolean, b: Boolean): Int = {x; 7}
  def boolean_local_read(x: Boolean, y: Boolean, b: Boolean): Int = {val z: Boolean = x; z; 7}
  def boolean_if_no_else(x: Boolean, y: Boolean, b: Boolean): Int = {var steps: Int = 0; if b then {steps = steps + 1; x}; steps}
  def boolean_while_body(x: Boolean, y: Boolean, b: Boolean): Int = {var steps: Int = 0; while steps < 2 do {steps = steps + 1; x}; steps}
  def boolean_effect_order(x: Boolean, y: Boolean, b: Boolean): Int = {var steps: Int = 0; {steps = steps * 10 + 1; x}; {steps = steps * 10 + 2; y}; steps}
  def boolean_match_statement(x: Boolean, y: Boolean, b: Boolean): Int = {var steps: Int = 0; b match {case true => {steps = 11; x}; case false => {steps = 22; y}}; steps}
  def boolean_preserve_result(x: Boolean, y: Boolean, b: Boolean): Boolean = {x; y}
  def boolean_selector_statement(x: Boolean, y: Boolean, b: Boolean): Int = {x; b} match {case true => 1;case false => 2}
  def boolean_guard_statement(x: Boolean, y: Boolean, b: Boolean): Int = b match {case n if {x; n} => 1;case _ => 2}
  def boolean_mixed_return(x: Boolean, y: Boolean, b: Boolean): Int = {if b then return 3; x; 7}
  def boolean_all_returning(x: Boolean, y: Boolean, b: Boolean): Int = {if b then return 3 else return 4; x; 7}
  def long_formal(x: Long, y: Long, b: Boolean): Unit = x
  def long_literal(x: Long, y: Long, b: Boolean): Unit = 1L
  def long_ascription(x: Long, y: Long, b: Boolean): Unit = (x: Long)
  def long_if_unit(x: Long, y: Long, b: Boolean): Unit = if b then x else y
  def long_match_unit(x: Long, y: Long, b: Boolean): Unit = b match {case true => x;case false => y}
  def long_return_value(x: Long, y: Long, b: Boolean): Unit = return x
  def long_block_unit(x: Long, y: Long, b: Boolean): Unit = {var n: Int = 0; n = n + 1; x}
  def long_mixed_unit(x: Long, y: Long, b: Boolean): Unit = {if b then return (); x}
  def long_nested_unit(x: Long, y: Long, b: Boolean): Unit = {{x; 1}; 1L}
  def long_statement(x: Long, y: Long, b: Boolean): Int = {x; 7}
  def long_local_read(x: Long, y: Long, b: Boolean): Int = {val z: Long = x; z; 7}
  def long_if_no_else(x: Long, y: Long, b: Boolean): Int = {var steps: Int = 0; if b then {steps = steps + 1; x}; steps}
  def long_while_body(x: Long, y: Long, b: Boolean): Int = {var steps: Int = 0; while steps < 2 do {steps = steps + 1; x}; steps}
  def long_effect_order(x: Long, y: Long, b: Boolean): Int = {var steps: Int = 0; {steps = steps * 10 + 1; x}; {steps = steps * 10 + 2; y}; steps}
  def long_match_statement(x: Long, y: Long, b: Boolean): Int = {var steps: Int = 0; b match {case true => {steps = 11; x}; case false => {steps = 22; y}}; steps}
  def long_preserve_result(x: Long, y: Long, b: Boolean): Long = {x; y}
  def long_selector_statement(x: Long, y: Long, b: Boolean): Int = {x; b} match {case true => 1;case false => 2}
  def long_guard_statement(x: Long, y: Long, b: Boolean): Int = b match {case n if {x; n} => 1;case _ => 2}
  def long_mixed_return(x: Long, y: Long, b: Boolean): Int = {if b then return 3; x; 7}
  def long_all_returning(x: Long, y: Long, b: Boolean): Int = {if b then return 3 else return 4; x; 7}
  def float_formal(x: Float, y: Float, b: Boolean): Unit = x
  def float_literal(x: Float, y: Float, b: Boolean): Unit = 1f
  def float_ascription(x: Float, y: Float, b: Boolean): Unit = (x: Float)
  def float_if_unit(x: Float, y: Float, b: Boolean): Unit = if b then x else y
  def float_match_unit(x: Float, y: Float, b: Boolean): Unit = b match {case true => x;case false => y}
  def float_return_value(x: Float, y: Float, b: Boolean): Unit = return x
  def float_block_unit(x: Float, y: Float, b: Boolean): Unit = {var n: Int = 0; n = n + 1; x}
  def float_mixed_unit(x: Float, y: Float, b: Boolean): Unit = {if b then return (); x}
  def float_nested_unit(x: Float, y: Float, b: Boolean): Unit = {{x; 1}; 1f}
  def float_statement(x: Float, y: Float, b: Boolean): Int = {x; 7}
  def float_local_read(x: Float, y: Float, b: Boolean): Int = {val z: Float = x; z; 7}
  def float_if_no_else(x: Float, y: Float, b: Boolean): Int = {var steps: Int = 0; if b then {steps = steps + 1; x}; steps}
  def float_while_body(x: Float, y: Float, b: Boolean): Int = {var steps: Int = 0; while steps < 2 do {steps = steps + 1; x}; steps}
  def float_effect_order(x: Float, y: Float, b: Boolean): Int = {var steps: Int = 0; {steps = steps * 10 + 1; x}; {steps = steps * 10 + 2; y}; steps}
  def float_match_statement(x: Float, y: Float, b: Boolean): Int = {var steps: Int = 0; b match {case true => {steps = 11; x}; case false => {steps = 22; y}}; steps}
  def float_preserve_result(x: Float, y: Float, b: Boolean): Float = {x; y}
  def float_selector_statement(x: Float, y: Float, b: Boolean): Int = {x; b} match {case true => 1;case false => 2}
  def float_guard_statement(x: Float, y: Float, b: Boolean): Int = b match {case n if {x; n} => 1;case _ => 2}
  def float_mixed_return(x: Float, y: Float, b: Boolean): Int = {if b then return 3; x; 7}
  def float_all_returning(x: Float, y: Float, b: Boolean): Int = {if b then return 3 else return 4; x; 7}
  def double_formal(x: Double, y: Double, b: Boolean): Unit = x
  def double_literal(x: Double, y: Double, b: Boolean): Unit = 1d
  def double_ascription(x: Double, y: Double, b: Boolean): Unit = (x: Double)
  def double_if_unit(x: Double, y: Double, b: Boolean): Unit = if b then x else y
  def double_match_unit(x: Double, y: Double, b: Boolean): Unit = b match {case true => x;case false => y}
  def double_return_value(x: Double, y: Double, b: Boolean): Unit = return x
  def double_block_unit(x: Double, y: Double, b: Boolean): Unit = {var n: Int = 0; n = n + 1; x}
  def double_mixed_unit(x: Double, y: Double, b: Boolean): Unit = {if b then return (); x}
  def double_nested_unit(x: Double, y: Double, b: Boolean): Unit = {{x; 1}; 1d}
  def double_statement(x: Double, y: Double, b: Boolean): Int = {x; 7}
  def double_local_read(x: Double, y: Double, b: Boolean): Int = {val z: Double = x; z; 7}
  def double_if_no_else(x: Double, y: Double, b: Boolean): Int = {var steps: Int = 0; if b then {steps = steps + 1; x}; steps}
  def double_while_body(x: Double, y: Double, b: Boolean): Int = {var steps: Int = 0; while steps < 2 do {steps = steps + 1; x}; steps}
  def double_effect_order(x: Double, y: Double, b: Boolean): Int = {var steps: Int = 0; {steps = steps * 10 + 1; x}; {steps = steps * 10 + 2; y}; steps}
  def double_match_statement(x: Double, y: Double, b: Boolean): Int = {var steps: Int = 0; b match {case true => {steps = 11; x}; case false => {steps = 22; y}}; steps}
  def double_preserve_result(x: Double, y: Double, b: Boolean): Double = {x; y}
  def double_selector_statement(x: Double, y: Double, b: Boolean): Int = {x; b} match {case true => 1;case false => 2}
  def double_guard_statement(x: Double, y: Double, b: Boolean): Int = b match {case n if {x; n} => 1;case _ => 2}
  def double_mixed_return(x: Double, y: Double, b: Boolean): Int = {if b then return 3; x; 7}
  def double_all_returning(x: Double, y: Double, b: Boolean): Int = {if b then return 3 else return 4; x; 7}
  def divide_unit(x: Int): Unit = 10 / x
  def divide_statement(x: Int): Int = {10 / x; 7}
  def divide_if(x: Int,b: Boolean): Int = {if b then 10 / x; 7}
  def divide_after_return(x: Int,b: Boolean): Int = {if b then return 3; 10 / x; 7}
}
