object ScalaConsumer {
var checks = 0
def check(a: Int,e: Int): Unit = {assert(a == e); checks += 1}
def check(a: Long,e: Long): Unit = {assert(a == e); checks += 1}
def check(a: Boolean,e: Boolean): Unit = {assert(a == e); checks += 1}
def check(a: Float,e: Float): Unit = {assert(if java.lang.Float.isNaN(e) then java.lang.Float.isNaN(a) else java.lang.Float.floatToRawIntBits(a) == java.lang.Float.floatToRawIntBits(e)); checks += 1}
def check(a: Double,e: Double): Unit = {assert(if java.lang.Double.isNaN(e) then java.lang.Double.isNaN(a) else java.lang.Double.doubleToRawLongBits(a) == java.lang.Double.doubleToRawLongBits(e)); checks += 1}
def throwsArithmetic(f: => Unit): Unit = {var thrown = false; try f catch {case _: ArithmeticException => thrown = true}; assert(thrown); checks += 1}
def main(args: Array[String]): Unit = {val p = new example.PrimitiveValueDiscard()
val ivalues = Array(Int.MinValue,-1,0,1,Int.MaxValue)
val bvalues = Array(false,true)
val lvalues = Array(Long.MinValue,-4294967295L,-1L,0L,1L,4294967297L,Long.MaxValue)
val fvalues = Array(Float.NegativeInfinity,-Float.MaxValue,-java.lang.Float.MIN_VALUE,-0.0f,0.0f,java.lang.Float.MIN_VALUE,java.lang.Float.MIN_NORMAL,1f,1.5f,java.lang.Math.nextUp(1.5f),2.5f,3.5f,Float.MaxValue,Float.PositiveInfinity,java.lang.Float.intBitsToFloat(0x7fc00042),java.lang.Float.intBitsToFloat(0xffc00042),java.lang.Float.intBitsToFloat(0x7f800042))
val dvalues = Array(Double.NegativeInfinity,-Double.MaxValue,-java.lang.Double.MIN_VALUE,-0.0d,0.0d,java.lang.Double.MIN_VALUE,java.lang.Double.MIN_NORMAL,1d,1.5d,java.lang.Math.nextUp(1.5d),2.5d,3.5d,Double.MaxValue,Double.PositiveInfinity,java.lang.Double.longBitsToDouble(0x7ff8000000000042L),java.lang.Double.longBitsToDouble(0xfff8000000000042L),java.lang.Double.longBitsToDouble(0x7ff0000000000042L))
for x <- ivalues; b <- Array(false,true) do { val y: Int = 17
p.int_formal(x,y,b); checks += 1
p.int_literal(x,y,b); checks += 1
p.int_ascription(x,y,b); checks += 1
p.int_if_unit(x,y,b); checks += 1
p.int_match_unit(x,y,b); checks += 1
p.int_return_value(x,y,b); checks += 1
p.int_block_unit(x,y,b); checks += 1
p.int_mixed_unit(x,y,b); checks += 1
p.int_nested_unit(x,y,b); checks += 1
check(p.int_statement(x,y,b),7)
check(p.int_local_read(x,y,b),7)
check(p.int_if_no_else(x,y,b),(if b then 1 else 0))
check(p.int_while_body(x,y,b),2)
check(p.int_effect_order(x,y,b),12)
check(p.int_match_statement(x,y,b),(if b then 11 else 22))
check(p.int_preserve_result(x,y,b),y)
check(p.int_selector_statement(x,y,b),(if b then 1 else 2))
check(p.int_guard_statement(x,y,b),(if b then 1 else 2))
check(p.int_mixed_return(x,y,b),(if b then 3 else 7))
check(p.int_all_returning(x,y,b),(if b then 3 else 4))
}
for x <- bvalues; b <- Array(false,true) do { val y: Boolean = true
p.boolean_formal(x,y,b); checks += 1
p.boolean_literal(x,y,b); checks += 1
p.boolean_ascription(x,y,b); checks += 1
p.boolean_if_unit(x,y,b); checks += 1
p.boolean_match_unit(x,y,b); checks += 1
p.boolean_return_value(x,y,b); checks += 1
p.boolean_block_unit(x,y,b); checks += 1
p.boolean_mixed_unit(x,y,b); checks += 1
p.boolean_nested_unit(x,y,b); checks += 1
check(p.boolean_statement(x,y,b),7)
check(p.boolean_local_read(x,y,b),7)
check(p.boolean_if_no_else(x,y,b),(if b then 1 else 0))
check(p.boolean_while_body(x,y,b),2)
check(p.boolean_effect_order(x,y,b),12)
check(p.boolean_match_statement(x,y,b),(if b then 11 else 22))
check(p.boolean_preserve_result(x,y,b),y)
check(p.boolean_selector_statement(x,y,b),(if b then 1 else 2))
check(p.boolean_guard_statement(x,y,b),(if b then 1 else 2))
check(p.boolean_mixed_return(x,y,b),(if b then 3 else 7))
check(p.boolean_all_returning(x,y,b),(if b then 3 else 4))
}
for x <- lvalues; b <- Array(false,true) do { val y: Long = 4294967297L
p.long_formal(x,y,b); checks += 1
p.long_literal(x,y,b); checks += 1
p.long_ascription(x,y,b); checks += 1
p.long_if_unit(x,y,b); checks += 1
p.long_match_unit(x,y,b); checks += 1
p.long_return_value(x,y,b); checks += 1
p.long_block_unit(x,y,b); checks += 1
p.long_mixed_unit(x,y,b); checks += 1
p.long_nested_unit(x,y,b); checks += 1
check(p.long_statement(x,y,b),7)
check(p.long_local_read(x,y,b),7)
check(p.long_if_no_else(x,y,b),(if b then 1 else 0))
check(p.long_while_body(x,y,b),2)
check(p.long_effect_order(x,y,b),12)
check(p.long_match_statement(x,y,b),(if b then 11 else 22))
check(p.long_preserve_result(x,y,b),y)
check(p.long_selector_statement(x,y,b),(if b then 1 else 2))
check(p.long_guard_statement(x,y,b),(if b then 1 else 2))
check(p.long_mixed_return(x,y,b),(if b then 3 else 7))
check(p.long_all_returning(x,y,b),(if b then 3 else 4))
}
for x <- fvalues; b <- Array(false,true) do { val y: Float = -0.0f
p.float_formal(x,y,b); checks += 1
p.float_literal(x,y,b); checks += 1
p.float_ascription(x,y,b); checks += 1
p.float_if_unit(x,y,b); checks += 1
p.float_match_unit(x,y,b); checks += 1
p.float_return_value(x,y,b); checks += 1
p.float_block_unit(x,y,b); checks += 1
p.float_mixed_unit(x,y,b); checks += 1
p.float_nested_unit(x,y,b); checks += 1
check(p.float_statement(x,y,b),7)
check(p.float_local_read(x,y,b),7)
check(p.float_if_no_else(x,y,b),(if b then 1 else 0))
check(p.float_while_body(x,y,b),2)
check(p.float_effect_order(x,y,b),12)
check(p.float_match_statement(x,y,b),(if b then 11 else 22))
check(p.float_preserve_result(x,y,b),y)
check(p.float_selector_statement(x,y,b),(if b then 1 else 2))
check(p.float_guard_statement(x,y,b),(if b then 1 else 2))
check(p.float_mixed_return(x,y,b),(if b then 3 else 7))
check(p.float_all_returning(x,y,b),(if b then 3 else 4))
}
for x <- dvalues; b <- Array(false,true) do { val y: Double = -0.0d
p.double_formal(x,y,b); checks += 1
p.double_literal(x,y,b); checks += 1
p.double_ascription(x,y,b); checks += 1
p.double_if_unit(x,y,b); checks += 1
p.double_match_unit(x,y,b); checks += 1
p.double_return_value(x,y,b); checks += 1
p.double_block_unit(x,y,b); checks += 1
p.double_mixed_unit(x,y,b); checks += 1
p.double_nested_unit(x,y,b); checks += 1
check(p.double_statement(x,y,b),7)
check(p.double_local_read(x,y,b),7)
check(p.double_if_no_else(x,y,b),(if b then 1 else 0))
check(p.double_while_body(x,y,b),2)
check(p.double_effect_order(x,y,b),12)
check(p.double_match_statement(x,y,b),(if b then 11 else 22))
check(p.double_preserve_result(x,y,b),y)
check(p.double_selector_statement(x,y,b),(if b then 1 else 2))
check(p.double_guard_statement(x,y,b),(if b then 1 else 2))
check(p.double_mixed_return(x,y,b),(if b then 3 else 7))
check(p.double_all_returning(x,y,b),(if b then 3 else 4))
}
p.divide_unit(2); checks += 1
throwsArithmetic(p.divide_unit(0))
check(p.divide_statement(2),7)
throwsArithmetic(p.divide_statement(0))
check(p.divide_if(0,false),7)
throwsArithmetic(p.divide_if(0,true))
check(p.divide_after_return(0,true),3)
throwsArithmetic(p.divide_after_return(0,false))
println("OK "+checks+" checks")
}
}
