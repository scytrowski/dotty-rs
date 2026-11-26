package example
class LongValues {
  private var trace: Int = 0
  def next(): Int = { trace = trace * 10 + 1; 5 }
  def guardCode(): Int = { trace = trace * 10 + 2; 0 }
  def observed(): Int = trace
  def literal(): Long = 9_000_000_000L
  def minimum(): Long = -9223372036854775808L
  def maximum(): Long = 9223372036854775807L
  def hexadecimal(): Long = 0xffffffffffffffffL
  def binary(): Long = 0b101010L
  def direct(x: Long): Long = x
  def mixed(a: Int, x: Long, flag: Boolean, y: Long): Long = if flag then x else y
  def intResult(x: Long): Int = 7
  def saved(x: Long): Long = { val value: Long = x; value }
  def inferred(x: Long): Long = { val value = x; value }
  def assigned(a: Long, b: Long): Long = { var value: Long = a; value = b; value }
  def rotate(n: Int, a: Long, b: Long): Long = {
    var value: Long = a
    var i: Int = n
    while i > 0 do { value = b; i = i - 1 }
    value
  }
  def choose(x: Int, a: Long, b: Long): Long = x match { case 0 => a; case _ => b }
  def guarded(x: Int, a: Long, b: Long): Long = x match { case n if n < 0 => a; case _ => b }
  def constants(x: Int): Long = x match { case 0 => 1L; case _ => 9000000000L }
  def once(a: Long, b: Long): Long = next() match { case 0 => a; case n if guardCode() > 0 => a; case _ => b }
  def early(flag: Boolean, a: Long, b: Long): Long = { if flag then return a; b }
  def both(flag: Boolean, a: Long, b: Long): Long = if flag then return a else return b
  def returned(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => b }
  def allReturn(x: Int, a: Long, b: Long): Long = x match { case 0 => return a; case _ => return b }
  def deadIf(flag: Boolean, a: Long, b: Long): Long = if flag then { return a; b } else b
  def deadMatch(x: Int, a: Long, b: Long): Long = x match { case 0 => { return a; b }; case _ => b }
  def inLoop(n: Int, a: Long, b: Long): Long = {
    var value: Long = a
    var i: Int = n
    while i > 0 do { if i == 3 then return a; value = b; i = i - 1 }
    value
  }
}
