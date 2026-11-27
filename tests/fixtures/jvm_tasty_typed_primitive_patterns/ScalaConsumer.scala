object ScalaConsumer {
var checks = 0
def check(a: Int,e: Int): Unit = { assert(a == e); checks += 1 }
def check(a: Boolean,e: Boolean): Unit = { assert(a == e); checks += 1 }
def check(a: Long,e: Long): Unit = { assert(a == e); checks += 1 }
def check(a: Float,e: Float): Unit = { assert(if java.lang.Float.isNaN(e) then java.lang.Float.isNaN(a) else java.lang.Float.floatToRawIntBits(a) == java.lang.Float.floatToRawIntBits(e)); checks += 1 }
def check(a: Double,e: Double): Unit = { assert(if java.lang.Double.isNaN(e) then java.lang.Double.isNaN(a) else java.lang.Double.doubleToRawLongBits(a) == java.lang.Double.doubleToRawLongBits(e)); checks += 1 }
def main(args: Array[String]): Unit = { val p = new example.TypedPrimitivePatterns()
for x <- Array(Int.MinValue,-1,0,1,2,Int.MaxValue); b <- Array(false,true) do {
check(p.int_wildcard(x,b),1)
check(p.int_binder(x,b),x)
check(p.int_explicit_binder(x,b),x)
check(p.int_literal_prefix(x,b),x)
check(p.int_guarded_binder(x,b),(if b then x else 0))
check(p.int_guarded_wildcard(x,b),(if b then 1 else 0))
check(p.int_two_binders(x,b),x)
check(p.int_nested(x,b),x)
check(p.int_typed_body(x,b),x)
check(p.int_local_value(x,b),x)
check(p.int_assignment(x,b),x)
check(p.int_returns(x,b),x)
check(p.int_result_int(x,b),7)
check(p.int_result_boolean(x,b),b)
p.int_result_unit(x,b); checks += 1
for a <- Array(0L,Long.MinValue,Long.MaxValue,1099511627776L) do check(p.int_result_long(x,b,a),a)
for a <- Array(0f,-0.0f,java.lang.Float.MIN_VALUE,java.lang.Float.MAX_VALUE,java.lang.Float.POSITIVE_INFINITY,java.lang.Float.NEGATIVE_INFINITY,java.lang.Float.intBitsToFloat(0x7fc12345)) do check(p.int_result_float(x,b,a),a)
for a <- Array(0.0,-0.0,java.lang.Double.MIN_VALUE,java.lang.Double.MAX_VALUE,java.lang.Double.POSITIVE_INFINITY,java.lang.Double.NEGATIVE_INFINITY,java.lang.Double.longBitsToDouble(0x7ff8000000012345L)) do check(p.int_result_double(x,b,a),a)
}
for x <- Array(false,true); b <- Array(false,true) do {
check(p.boolean_wildcard(x,b),true)
check(p.boolean_binder(x,b),x)
check(p.boolean_explicit_binder(x,b),x)
check(p.boolean_literal_prefix(x,b),x)
check(p.boolean_guarded_binder(x,b),(if b then x else false))
check(p.boolean_guarded_wildcard(x,b),(if b then true else false))
check(p.boolean_two_binders(x,b),x)
check(p.boolean_nested(x,b),x)
check(p.boolean_typed_body(x,b),x)
check(p.boolean_local_value(x,b),x)
check(p.boolean_assignment(x,b),x)
check(p.boolean_returns(x,b),x)
check(p.boolean_result_int(x,b),7)
check(p.boolean_result_boolean(x,b),b)
p.boolean_result_unit(x,b); checks += 1
for a <- Array(0L,Long.MinValue,Long.MaxValue,1099511627776L) do check(p.boolean_result_long(x,b,a),a)
for a <- Array(0f,-0.0f,java.lang.Float.MIN_VALUE,java.lang.Float.MAX_VALUE,java.lang.Float.POSITIVE_INFINITY,java.lang.Float.NEGATIVE_INFINITY,java.lang.Float.intBitsToFloat(0x7fc12345)) do check(p.boolean_result_float(x,b,a),a)
for a <- Array(0.0,-0.0,java.lang.Double.MIN_VALUE,java.lang.Double.MAX_VALUE,java.lang.Double.POSITIVE_INFINITY,java.lang.Double.NEGATIVE_INFINITY,java.lang.Double.longBitsToDouble(0x7ff8000000012345L)) do check(p.boolean_result_double(x,b,a),a)
}
for x <- Array(-2,0,1,7); b <- Array(false,true) do { check(p.selector_effect(x,b),(if b then 123 else 124)*100+x); check(p.guard_skipped(x,b),(if x == 0 then (if b then 123 else 124) else 14)*100+x) }
for x <- Array(false,true); b <- Array(false,true) do check(p.boolean_effect(x,b),(if b then 123 else 124)*100+(if x then 10 else 20))
for x <- Array(Int.MinValue,-1,0,1,2,Int.MaxValue) do { for b <- Array(false,true) do check(p.shadow(x,b),x); check(p.parenthesized(x),1) }
for x <- Array(Int.MinValue,-1,0,1,2,Int.MaxValue) do check(p.binder_guard_int(x), if x > 0 then x else 0); for x <- Array(false,true) do check(p.binder_guard_boolean(x), if x then 1 else 0)
check(p.singleton_int(),1); check(p.singleton_boolean(),true)
assert(checks == 576); println(s"OK $checks checks")
}}
