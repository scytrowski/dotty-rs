import example.NumericAscriptions;
public class JavaConsumer {
static int checks = 0;
static void check(long a,long e){if(a!=e)throw new AssertionError(a+" != "+e);checks++;}
static void check(float a,float e){if(Float.isNaN(e)?!Float.isNaN(a):Float.floatToRawIntBits(a)!=Float.floatToRawIntBits(e))throw new AssertionError(a+" != "+e);checks++;}
static void check(double a,double e){if(Double.isNaN(e)?!Double.isNaN(a):Double.doubleToRawLongBits(a)!=Double.doubleToRawLongBits(e))throw new AssertionError(a+" != "+e);checks++;}
public static void main(String[] args){ NumericAscriptions p = new NumericAscriptions();
for (long x : new long[]{0L,1L,-1L,Long.MIN_VALUE,Long.MAX_VALUE,1099511627776L}) for(boolean b : new boolean[]{false,true}) for(int n : new int[]{-1,0,1,3}) {
check(p.long_identity(x,b,n),x);
check(p.long_literal(x,b,n),7L);
check(p.long_nested(x,b,n),x);
check(p.long_local_explicit(x,b,n),x);
check(p.long_local_inferred(x,b,n),x);
check(p.long_local_read(x,b,n),x);
check(p.long_assignment(x,b,n),x);
check(p.long_if_inside(x,b,n),(b ? x : 7L));
check(p.long_if_branches(x,b,n),(b ? x : 7L));
check(p.long_block_inside(x,b,n),x);
check(p.long_int_match_inside(x,b,n),(n == 1 ? x : 7L));
check(p.long_bool_match_inside(x,b,n),(b ? x : 7L));
check(p.long_bool_match_bodies(x,b,n),(b ? x : 7L));
check(p.long_return_value(x,b,n),x);
check(p.long_return_branch(x,b,n),(b ? x : 7L));
check(p.long_return_match(x,b,n),(b ? x : 7L));
check(p.long_effects(x,b,n),x);
check(p.long_loop(x,b,n),(n > 0 ? 7L : x));
}
for (float x : new float[]{0.0f,-0.0f,Float.MIN_VALUE,Float.MAX_VALUE,Float.POSITIVE_INFINITY,Float.NEGATIVE_INFINITY,Float.intBitsToFloat(0x7fc12345)}) for(boolean b : new boolean[]{false,true}) for(int n : new int[]{-1,0,1,3}) {
check(p.float_identity(x,b,n),x);
check(p.float_literal(x,b,n),0.5f);
check(p.float_nested(x,b,n),x);
check(p.float_local_explicit(x,b,n),x);
check(p.float_local_inferred(x,b,n),x);
check(p.float_local_read(x,b,n),x);
check(p.float_assignment(x,b,n),x);
check(p.float_if_inside(x,b,n),(b ? x : 0.5f));
check(p.float_if_branches(x,b,n),(b ? x : 0.5f));
check(p.float_block_inside(x,b,n),x);
check(p.float_int_match_inside(x,b,n),(n == 1 ? x : 0.5f));
check(p.float_bool_match_inside(x,b,n),(b ? x : 0.5f));
check(p.float_bool_match_bodies(x,b,n),(b ? x : 0.5f));
check(p.float_return_value(x,b,n),x);
check(p.float_return_branch(x,b,n),(b ? x : 0.5f));
check(p.float_return_match(x,b,n),(b ? x : 0.5f));
check(p.float_effects(x,b,n),x);
check(p.float_loop(x,b,n),(n > 0 ? 0.5f : x));
}
for (double x : new double[]{0.0,-0.0,Double.MIN_VALUE,Double.MAX_VALUE,Double.POSITIVE_INFINITY,Double.NEGATIVE_INFINITY,Double.longBitsToDouble(0x7ff8000000012345L)}) for(boolean b : new boolean[]{false,true}) for(int n : new int[]{-1,0,1,3}) {
check(p.double_identity(x,b,n),x);
check(p.double_literal(x,b,n),0.5);
check(p.double_nested(x,b,n),x);
check(p.double_local_explicit(x,b,n),x);
check(p.double_local_inferred(x,b,n),x);
check(p.double_local_read(x,b,n),x);
check(p.double_assignment(x,b,n),x);
check(p.double_if_inside(x,b,n),(b ? x : 0.5));
check(p.double_if_branches(x,b,n),(b ? x : 0.5));
check(p.double_block_inside(x,b,n),x);
check(p.double_int_match_inside(x,b,n),(n == 1 ? x : 0.5));
check(p.double_bool_match_inside(x,b,n),(b ? x : 0.5));
check(p.double_bool_match_bodies(x,b,n),(b ? x : 0.5));
check(p.double_return_value(x,b,n),x);
check(p.double_return_branch(x,b,n),(b ? x : 0.5));
check(p.double_return_match(x,b,n),(b ? x : 0.5));
check(p.double_effects(x,b,n),x);
check(p.double_loop(x,b,n),(n > 0 ? 0.5 : x));
}
check(p.minL(),Long.MIN_VALUE);
check(p.maxL(),Long.MAX_VALUE);
check(p.zeroF(),-0.0f);
check(p.tinyF(),Float.MIN_VALUE);
check(p.maxF(),Float.MAX_VALUE);
check(p.zeroD(),-0.0);
check(p.tinyD(),Double.MIN_VALUE);
check(p.maxD(),Double.MAX_VALUE);
System.out.println("OK "+checks+" checks");
}}
