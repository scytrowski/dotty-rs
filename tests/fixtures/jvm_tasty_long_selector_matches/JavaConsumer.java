import example.LongSelectorMatches;
public class JavaConsumer {
static int checks;
static void check(long a,long e){if(a!=e)throw new AssertionError(a+" != "+e);checks++;}
static void check(boolean a,boolean e){if(a!=e)throw new AssertionError();checks++;}
static void check(float a,float e){if(Float.floatToRawIntBits(a)!=Float.floatToRawIntBits(e))throw new AssertionError();checks++;}
static void check(double a,double e){if(Double.doubleToRawLongBits(a)!=Double.doubleToRawLongBits(e))throw new AssertionError();checks++;}
public static void main(String[] args){var p=new LongSelectorMatches();
for(long x:new long[]{Long.MIN_VALUE,-4294967295L,-1L,0L,1L,2L,2147483647L,2147483648L,4294967296L,4294967297L,Long.MAX_VALUE}) for(boolean b:new boolean[]{false,true}) {
check(p.literal(x,b,123L,987L),x == 1L ? 123L : 987L);
check(p.high_literal(x,b,123L,987L),x == 4294967297L ? 123L : 987L);
check(p.extremes(x,b,123L,987L),x == Long.MAX_VALUE ? 987L : 123L);
check(p.alternative(x,b,123L,987L),(x == 1L || x == 4294967297L) ? 123L : 987L);
check(p.nested_alternative(x,b,123L,987L),(x == 1L || x == 4294967297L || x == -1L) ? 123L : 987L);
check(p.guard(x,b,123L,987L),(x == 1L && b) ? 123L : 987L);
check(p.alternative_guard(x,b,123L,987L),((x == 1L || x == 4294967297L) && b) ? 123L : 987L);
check(p.binder(x,b,123L,987L),x);
check(p.bound_literal(x,b,123L,987L),x);
check(p.typed_binder(x,b,123L,987L),x);
check(p.typed_wildcard(x,b,123L,987L),123L);
check(p.explicit_typed_binder(x,b,123L,987L),x);
check(p.guarded_binder(x,b,123L,987L),x);
check(p.guarded_typed(x,b,123L,987L),b ? x : 123L);
check(p.nested(x,b,123L,987L),x == 4294967297L ? 123L : x);
check(p.typed_body(x,b,123L,987L),x);
check(p.returns(x,b,123L,987L),x);
check(p.result_Int_normal(x,b,7),x == 1L ? 7 : 0);
check(p.result_Int_guarded(x,b,7),b ? 7 : 0);
check(p.result_Int_returns(x,b,7),x == 1L ? 7 : 0);
check(p.result_Boolean_normal(x,b,true),x == 1L ? true : false);
check(p.result_Boolean_guarded(x,b,true),b ? true : false);
check(p.result_Boolean_returns(x,b,true),x == 1L ? true : false);
p.result_Unit_normal(x,b); checks++;
p.result_Unit_guarded(x,b); checks++;
p.result_Unit_returns(x,b); checks++;
check(p.result_Long_normal(x,b,Long.MAX_VALUE),x == 1L ? Long.MAX_VALUE : 0L);
check(p.result_Long_guarded(x,b,Long.MAX_VALUE),b ? Long.MAX_VALUE : 0L);
check(p.result_Long_returns(x,b,Long.MAX_VALUE),x == 1L ? Long.MAX_VALUE : 0L);
check(p.result_Float_normal(x,b,-0.0f),x == 1L ? -0.0f : 0.0f);
check(p.result_Float_guarded(x,b,-0.0f),b ? -0.0f : 0.0f);
check(p.result_Float_returns(x,b,-0.0f),x == 1L ? -0.0f : 0.0f);
check(p.result_Double_normal(x,b,-0.0d),x == 1L ? -0.0d : 0.0d);
check(p.result_Double_guarded(x,b,-0.0d),b ? -0.0d : 0.0d);
check(p.result_Double_returns(x,b,-0.0d),x == 1L ? -0.0d : 0.0d);
check(p.selector_local(x),x);
check(p.selector_mutable(x),x);
check(p.singleton(),4294967297L);
check(p.singleton_literal(),7L);
check(p.ascribed_selector(x),x);
check(p.local_result(x),x);
check(p.assignment_result(x),x);
check(p.effect(x,b),(x == 1L || x == 4294967297L) ? (b ? 12307 : 12408) : 1408);
check(p.shadow(x,b),x);
check(p.mixed_nested(x,b),b ? x : (x == 1L ? 7L : x));
check(p.plain_singleton(),1L);
check(p.bound_singleton(),1L);
check(p.nested_singleton(),1L);
check(p.inferred_local(x),x);
check(p.while_selector(x,b),b ? x : 0L);
check(p.non_tail_return(x,b),b ? x : 0L);
}
System.out.println("OK "+checks+" checks");
}
}
