package example
class NumericAscriptions {
  def long_identity(x: Long, b: Boolean, n: Int): Long = (x: Long)
  def long_literal(x: Long, b: Boolean, n: Int): Long = (7L: Long)
  def long_nested(x: Long, b: Boolean, n: Int): Long = (((x: Long)): Long)
  def long_local_explicit(x: Long, b: Boolean, n: Int): Long = { val a: Long = (x: Long); a }
  def long_local_inferred(x: Long, b: Boolean, n: Int): Long = { val a = (x: Long); a }
  def long_local_read(x: Long, b: Boolean, n: Int): Long = { val a = x; (a: Long) }
  def long_assignment(x: Long, b: Boolean, n: Int): Long = { var a: Long = 7L; a = (x: Long); a }
  def long_if_inside(x: Long, b: Boolean, n: Int): Long = ((if b then x else 7L): Long)
  def long_if_branches(x: Long, b: Boolean, n: Int): Long = if b then (x: Long) else (7L: Long)
  def long_block_inside(x: Long, b: Boolean, n: Int): Long = ({ val a = x; a }: Long)
  def long_int_match_inside(x: Long, b: Boolean, n: Int): Long = ((n match { case 1 => x; case _ => 7L }): Long)
  def long_bool_match_inside(x: Long, b: Boolean, n: Int): Long = ((b match { case true => x; case false => 7L }): Long)
  def long_bool_match_bodies(x: Long, b: Boolean, n: Int): Long = b match { case true => (x: Long); case false => (7L: Long) }
  def long_return_value(x: Long, b: Boolean, n: Int): Long = { return (x: Long) }
  def long_return_branch(x: Long, b: Boolean, n: Int): Long = if b then return (x: Long) else (7L: Long)
  def long_return_match(x: Long, b: Boolean, n: Int): Long = b match { case true => return (x: Long); case false => (7L: Long) }
  def long_effects(x: Long, b: Boolean, n: Int): Long = { var count = 0; val a = ({ count = count + 1; x }: Long); if count == 1 then a else 7L }
  def long_loop(x: Long, b: Boolean, n: Int): Long = { var a = (x: Long); var i = 0; while i < n do { a = (7L: Long); i = i + 1 }; (a: Long) }
  def float_identity(x: Float, b: Boolean, n: Int): Float = (x: Float)
  def float_literal(x: Float, b: Boolean, n: Int): Float = (0.5f: Float)
  def float_nested(x: Float, b: Boolean, n: Int): Float = (((x: Float)): Float)
  def float_local_explicit(x: Float, b: Boolean, n: Int): Float = { val a: Float = (x: Float); a }
  def float_local_inferred(x: Float, b: Boolean, n: Int): Float = { val a = (x: Float); a }
  def float_local_read(x: Float, b: Boolean, n: Int): Float = { val a = x; (a: Float) }
  def float_assignment(x: Float, b: Boolean, n: Int): Float = { var a: Float = 0.5f; a = (x: Float); a }
  def float_if_inside(x: Float, b: Boolean, n: Int): Float = ((if b then x else 0.5f): Float)
  def float_if_branches(x: Float, b: Boolean, n: Int): Float = if b then (x: Float) else (0.5f: Float)
  def float_block_inside(x: Float, b: Boolean, n: Int): Float = ({ val a = x; a }: Float)
  def float_int_match_inside(x: Float, b: Boolean, n: Int): Float = ((n match { case 1 => x; case _ => 0.5f }): Float)
  def float_bool_match_inside(x: Float, b: Boolean, n: Int): Float = ((b match { case true => x; case false => 0.5f }): Float)
  def float_bool_match_bodies(x: Float, b: Boolean, n: Int): Float = b match { case true => (x: Float); case false => (0.5f: Float) }
  def float_return_value(x: Float, b: Boolean, n: Int): Float = { return (x: Float) }
  def float_return_branch(x: Float, b: Boolean, n: Int): Float = if b then return (x: Float) else (0.5f: Float)
  def float_return_match(x: Float, b: Boolean, n: Int): Float = b match { case true => return (x: Float); case false => (0.5f: Float) }
  def float_effects(x: Float, b: Boolean, n: Int): Float = { var count = 0; val a = ({ count = count + 1; x }: Float); if count == 1 then a else 0.5f }
  def float_loop(x: Float, b: Boolean, n: Int): Float = { var a = (x: Float); var i = 0; while i < n do { a = (0.5f: Float); i = i + 1 }; (a: Float) }
  def double_identity(x: Double, b: Boolean, n: Int): Double = (x: Double)
  def double_literal(x: Double, b: Boolean, n: Int): Double = (0.5: Double)
  def double_nested(x: Double, b: Boolean, n: Int): Double = (((x: Double)): Double)
  def double_local_explicit(x: Double, b: Boolean, n: Int): Double = { val a: Double = (x: Double); a }
  def double_local_inferred(x: Double, b: Boolean, n: Int): Double = { val a = (x: Double); a }
  def double_local_read(x: Double, b: Boolean, n: Int): Double = { val a = x; (a: Double) }
  def double_assignment(x: Double, b: Boolean, n: Int): Double = { var a: Double = 0.5; a = (x: Double); a }
  def double_if_inside(x: Double, b: Boolean, n: Int): Double = ((if b then x else 0.5): Double)
  def double_if_branches(x: Double, b: Boolean, n: Int): Double = if b then (x: Double) else (0.5: Double)
  def double_block_inside(x: Double, b: Boolean, n: Int): Double = ({ val a = x; a }: Double)
  def double_int_match_inside(x: Double, b: Boolean, n: Int): Double = ((n match { case 1 => x; case _ => 0.5 }): Double)
  def double_bool_match_inside(x: Double, b: Boolean, n: Int): Double = ((b match { case true => x; case false => 0.5 }): Double)
  def double_bool_match_bodies(x: Double, b: Boolean, n: Int): Double = b match { case true => (x: Double); case false => (0.5: Double) }
  def double_return_value(x: Double, b: Boolean, n: Int): Double = { return (x: Double) }
  def double_return_branch(x: Double, b: Boolean, n: Int): Double = if b then return (x: Double) else (0.5: Double)
  def double_return_match(x: Double, b: Boolean, n: Int): Double = b match { case true => return (x: Double); case false => (0.5: Double) }
  def double_effects(x: Double, b: Boolean, n: Int): Double = { var count = 0; val a = ({ count = count + 1; x }: Double); if count == 1 then a else 0.5 }
  def double_loop(x: Double, b: Boolean, n: Int): Double = { var a = (x: Double); var i = 0; while i < n do { a = (0.5: Double); i = i + 1 }; (a: Double) }
  def minL(): Long = (-9223372036854775808L: Long)
  def maxL(): Long = (9223372036854775807L: Long)
  def zeroF(): Float = (-0.0f: Float)
  def tinyF(): Float = (1.4e-45f: Float)
  def maxF(): Float = (3.4028235e38f: Float)
  def zeroD(): Double = (-0.0: Double)
  def tinyD(): Double = (4.9e-324: Double)
  def maxD(): Double = (1.7976931348623157e308: Double)
}
