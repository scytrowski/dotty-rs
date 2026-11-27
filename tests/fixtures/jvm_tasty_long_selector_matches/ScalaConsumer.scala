object ScalaConsumer {
var checks = 0
def check(a: Long,e: Long): Unit = {assert(a == e); checks += 1}
def check(a: Int,e: Int): Unit = {assert(a == e); checks += 1}
def check(a: Boolean,e: Boolean): Unit = {assert(a == e); checks += 1}
def check(a: Float,e: Float): Unit = {assert(java.lang.Float.floatToRawIntBits(a) == java.lang.Float.floatToRawIntBits(e)); checks += 1}
def check(a: Double,e: Double): Unit = {assert(java.lang.Double.doubleToRawLongBits(a) == java.lang.Double.doubleToRawLongBits(e)); checks += 1}
def main(args: Array[String]): Unit = {val p = new example.LongSelectorMatches()
for x <- Array(Long.MinValue,-4294967295L,-1L,0L,1L,2L,2147483647L,2147483648L,4294967296L,4294967297L,Long.MaxValue); b <- Array(false,true) do {
check(p.literal(x,b,123L,987L),if x == 1L then 123L else 987L)
check(p.high_literal(x,b,123L,987L),if x == 4294967297L then 123L else 987L)
check(p.extremes(x,b,123L,987L),if x == Long.MaxValue then 987L else 123L)
check(p.alternative(x,b,123L,987L),if (x == 1L || x == 4294967297L) then 123L else 987L)
check(p.nested_alternative(x,b,123L,987L),if (x == 1L || x == 4294967297L || x == -1L) then 123L else 987L)
check(p.guard(x,b,123L,987L),if (x == 1L && b) then 123L else 987L)
check(p.alternative_guard(x,b,123L,987L),if ((x == 1L || x == 4294967297L) && b) then 123L else 987L)
check(p.binder(x,b,123L,987L),x)
check(p.bound_literal(x,b,123L,987L),x)
check(p.typed_binder(x,b,123L,987L),x)
check(p.typed_wildcard(x,b,123L,987L),123L)
check(p.explicit_typed_binder(x,b,123L,987L),x)
check(p.guarded_binder(x,b,123L,987L),x)
check(p.guarded_typed(x,b,123L,987L),if b then x else 123L)
check(p.nested(x,b,123L,987L),if x == 4294967297L then 123L else x)
check(p.typed_body(x,b,123L,987L),x)
check(p.returns(x,b,123L,987L),x)
check(p.result_Int_normal(x,b,7),if x == 1L then 7 else 0)
check(p.result_Int_guarded(x,b,7),if b then 7 else 0)
check(p.result_Int_returns(x,b,7),if x == 1L then 7 else 0)
check(p.result_Boolean_normal(x,b,true),if x == 1L then true else false)
check(p.result_Boolean_guarded(x,b,true),if b then true else false)
check(p.result_Boolean_returns(x,b,true),if x == 1L then true else false)
p.result_Unit_normal(x,b); checks += 1
p.result_Unit_guarded(x,b); checks += 1
p.result_Unit_returns(x,b); checks += 1
check(p.result_Long_normal(x,b,Long.MaxValue),if x == 1L then Long.MaxValue else 0L)
check(p.result_Long_guarded(x,b,Long.MaxValue),if b then Long.MaxValue else 0L)
check(p.result_Long_returns(x,b,Long.MaxValue),if x == 1L then Long.MaxValue else 0L)
check(p.result_Float_normal(x,b,-0.0f),if x == 1L then -0.0f else 0.0f)
check(p.result_Float_guarded(x,b,-0.0f),if b then -0.0f else 0.0f)
check(p.result_Float_returns(x,b,-0.0f),if x == 1L then -0.0f else 0.0f)
check(p.result_Double_normal(x,b,-0.0d),if x == 1L then -0.0d else 0.0d)
check(p.result_Double_guarded(x,b,-0.0d),if b then -0.0d else 0.0d)
check(p.result_Double_returns(x,b,-0.0d),if x == 1L then -0.0d else 0.0d)
check(p.selector_local(x),x)
check(p.selector_mutable(x),x)
check(p.singleton(),4294967297L)
check(p.singleton_literal(),7L)
check(p.ascribed_selector(x),x)
check(p.local_result(x),x)
check(p.assignment_result(x),x)
check(p.effect(x,b),if x == 1L || x == 4294967297L then (if b then 12307 else 12408) else 1408)
check(p.shadow(x,b),x)
check(p.mixed_nested(x,b),if b then x else if x == 1L then 7L else x)
check(p.plain_singleton(),1L)
check(p.bound_singleton(),1L)
check(p.nested_singleton(),1L)
check(p.inferred_local(x),x)
check(p.while_selector(x,b),if b then x else 0L)
check(p.non_tail_return(x,b),if b then x else 0L)
}
println("OK "+checks+" checks")
}
}
