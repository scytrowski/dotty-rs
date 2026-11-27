import example.TypedPrimitivePatterns;
public class JavaConsumer {
static int checks = 0;
static void check(int a,int e){if(a!=e)throw new AssertionError(a+" != "+e);checks++;}
static void check(boolean a,boolean e){if(a!=e)throw new AssertionError();checks++;}
static void check(long a,long e){if(a!=e)throw new AssertionError();checks++;}
static void check(float a,float e){if(Float.isNaN(e)?!Float.isNaN(a):Float.floatToRawIntBits(a)!=Float.floatToRawIntBits(e))throw new AssertionError();checks++;}
static void check(double a,double e){if(Double.isNaN(e)?!Double.isNaN(a):Double.doubleToRawLongBits(a)!=Double.doubleToRawLongBits(e))throw new AssertionError();checks++;}
public static void main(String[] args){ TypedPrimitivePatterns p = new TypedPrimitivePatterns();
for (int x : new int[]{Integer.MIN_VALUE,-1,0,1,2,Integer.MAX_VALUE}) for(boolean b : new boolean[]{false,true}) {
check(p.int_wildcard(x,b),1);
check(p.int_binder(x,b),x);
check(p.int_explicit_binder(x,b),x);
check(p.int_literal_prefix(x,b),x);
check(p.int_guarded_binder(x,b),(b ? x : 0));
check(p.int_guarded_wildcard(x,b),(b ? 1 : 0));
check(p.int_two_binders(x,b),x);
check(p.int_nested(x,b),x);
check(p.int_typed_body(x,b),x);
check(p.int_local_value(x,b),x);
check(p.int_assignment(x,b),x);
check(p.int_returns(x,b),x);
check(p.int_result_int(x,b),7);
check(p.int_result_boolean(x,b),b);
p.int_result_unit(x,b); checks++;
for (long a : new long[]{0L,Long.MIN_VALUE,Long.MAX_VALUE,1099511627776L}) check(p.int_result_long(x,b,a),a);
for (float a : new float[]{0f,-0f,Float.MIN_VALUE,Float.MAX_VALUE,Float.POSITIVE_INFINITY,Float.NEGATIVE_INFINITY,Float.intBitsToFloat(0x7fc12345)}) check(p.int_result_float(x,b,a),a);
for (double a : new double[]{0.0,-0.0,Double.MIN_VALUE,Double.MAX_VALUE,Double.POSITIVE_INFINITY,Double.NEGATIVE_INFINITY,Double.longBitsToDouble(0x7ff8000000012345L)}) check(p.int_result_double(x,b,a),a);
}
for (boolean x : new boolean[]{false,true}) for(boolean b : new boolean[]{false,true}) {
check(p.boolean_wildcard(x,b),true);
check(p.boolean_binder(x,b),x);
check(p.boolean_explicit_binder(x,b),x);
check(p.boolean_literal_prefix(x,b),x);
check(p.boolean_guarded_binder(x,b),(b ? x : false));
check(p.boolean_guarded_wildcard(x,b),(b ? true : false));
check(p.boolean_two_binders(x,b),x);
check(p.boolean_nested(x,b),x);
check(p.boolean_typed_body(x,b),x);
check(p.boolean_local_value(x,b),x);
check(p.boolean_assignment(x,b),x);
check(p.boolean_returns(x,b),x);
check(p.boolean_result_int(x,b),7);
check(p.boolean_result_boolean(x,b),b);
p.boolean_result_unit(x,b); checks++;
for (long a : new long[]{0L,Long.MIN_VALUE,Long.MAX_VALUE,1099511627776L}) check(p.boolean_result_long(x,b,a),a);
for (float a : new float[]{0f,-0f,Float.MIN_VALUE,Float.MAX_VALUE,Float.POSITIVE_INFINITY,Float.NEGATIVE_INFINITY,Float.intBitsToFloat(0x7fc12345)}) check(p.boolean_result_float(x,b,a),a);
for (double a : new double[]{0.0,-0.0,Double.MIN_VALUE,Double.MAX_VALUE,Double.POSITIVE_INFINITY,Double.NEGATIVE_INFINITY,Double.longBitsToDouble(0x7ff8000000012345L)}) check(p.boolean_result_double(x,b,a),a);
}
for(int x : new int[]{-2,0,1,7}) for(boolean b : new boolean[]{false,true}) { check(p.selector_effect(x,b),(b?123:124)*100+x); check(p.guard_skipped(x,b),(x==0?(b?123:124):14)*100+x); }
for(boolean x : new boolean[]{false,true}) for(boolean b : new boolean[]{false,true}) check(p.boolean_effect(x,b),(b?123:124)*100+(x?10:20));
for(int x : new int[]{Integer.MIN_VALUE,-1,0,1,2,Integer.MAX_VALUE}) { for(boolean b : new boolean[]{false,true}) check(p.shadow(x,b),x); check(p.parenthesized(x),1); }
for(int x : new int[]{Integer.MIN_VALUE,-1,0,1,2,Integer.MAX_VALUE}) check(p.binder_guard_int(x), x>0?x:0); for(boolean x : new boolean[]{false,true}) check(p.binder_guard_boolean(x),x?1:0);
check(p.singleton_int(),1); check(p.singleton_boolean(),true);
if(checks!=576)throw new AssertionError(checks); System.out.println("OK "+checks+" checks");
}}
