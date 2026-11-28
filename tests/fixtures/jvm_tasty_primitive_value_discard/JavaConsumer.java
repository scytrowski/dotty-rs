import example.PrimitiveValueDiscard;
public class JavaConsumer {
static int checks;
static void check(int a,int e){if(a!=e)throw new AssertionError(a+" != "+e);checks++;}
static void check(long a,long e){if(a!=e)throw new AssertionError(a+" != "+e);checks++;}
static void check(boolean a,boolean e){if(a!=e)throw new AssertionError();checks++;}
static void check(float a,float e){if(Float.isNaN(e) ? !Float.isNaN(a) : Float.floatToRawIntBits(a)!=Float.floatToRawIntBits(e))throw new AssertionError(a+" != "+e);checks++;}
static void check(double a,double e){if(Double.isNaN(e) ? !Double.isNaN(a) : Double.doubleToRawLongBits(a)!=Double.doubleToRawLongBits(e))throw new AssertionError(a+" != "+e);checks++;}
static void throwsArithmetic(Runnable f){boolean thrown=false;try{f.run();}catch(ArithmeticException e){thrown=true;}if(!thrown)throw new AssertionError("lost evaluation");checks++;}
public static void main(String[] args){var p=new PrimitiveValueDiscard();
int[] ivalues={Integer.MIN_VALUE,-1,0,1,Integer.MAX_VALUE};
boolean[] bvalues={false,true};
long[] lvalues={Long.MIN_VALUE,-4294967295L,-1L,0L,1L,4294967297L,Long.MAX_VALUE};
float[] fvalues={Float.NEGATIVE_INFINITY,-Float.MAX_VALUE,-Float.MIN_VALUE,-0.0f,0.0f,Float.MIN_VALUE,Float.MIN_NORMAL,1f,1.5f,Math.nextUp(1.5f),2.5f,3.5f,Float.MAX_VALUE,Float.POSITIVE_INFINITY,Float.intBitsToFloat(0x7fc00042),Float.intBitsToFloat(0xffc00042),Float.intBitsToFloat(0x7f800042)};
double[] dvalues={Double.NEGATIVE_INFINITY,-Double.MAX_VALUE,-Double.MIN_VALUE,-0.0d,0.0d,Double.MIN_VALUE,Double.MIN_NORMAL,1d,1.5d,Math.nextUp(1.5d),2.5d,3.5d,Double.MAX_VALUE,Double.POSITIVE_INFINITY,Double.longBitsToDouble(0x7ff8000000000042L),Double.longBitsToDouble(0xfff8000000000042L),Double.longBitsToDouble(0x7ff0000000000042L)};
for (int x : ivalues) for (boolean b : new boolean[]{false,true}) { int y = 17;
p.int_formal(x,y,b); checks++;
p.int_literal(x,y,b); checks++;
p.int_ascription(x,y,b); checks++;
p.int_if_unit(x,y,b); checks++;
p.int_match_unit(x,y,b); checks++;
p.int_return_value(x,y,b); checks++;
p.int_block_unit(x,y,b); checks++;
p.int_mixed_unit(x,y,b); checks++;
p.int_nested_unit(x,y,b); checks++;
check(p.int_statement(x,y,b),7);
check(p.int_local_read(x,y,b),7);
check(p.int_if_no_else(x,y,b),b ? 1 : 0);
check(p.int_while_body(x,y,b),2);
check(p.int_effect_order(x,y,b),12);
check(p.int_match_statement(x,y,b),b ? 11 : 22);
check(p.int_preserve_result(x,y,b),y);
check(p.int_selector_statement(x,y,b),b ? 1 : 2);
check(p.int_guard_statement(x,y,b),b ? 1 : 2);
check(p.int_mixed_return(x,y,b),b ? 3 : 7);
check(p.int_all_returning(x,y,b),b ? 3 : 4);
}
for (boolean x : bvalues) for (boolean b : new boolean[]{false,true}) { boolean y = true;
p.boolean_formal(x,y,b); checks++;
p.boolean_literal(x,y,b); checks++;
p.boolean_ascription(x,y,b); checks++;
p.boolean_if_unit(x,y,b); checks++;
p.boolean_match_unit(x,y,b); checks++;
p.boolean_return_value(x,y,b); checks++;
p.boolean_block_unit(x,y,b); checks++;
p.boolean_mixed_unit(x,y,b); checks++;
p.boolean_nested_unit(x,y,b); checks++;
check(p.boolean_statement(x,y,b),7);
check(p.boolean_local_read(x,y,b),7);
check(p.boolean_if_no_else(x,y,b),b ? 1 : 0);
check(p.boolean_while_body(x,y,b),2);
check(p.boolean_effect_order(x,y,b),12);
check(p.boolean_match_statement(x,y,b),b ? 11 : 22);
check(p.boolean_preserve_result(x,y,b),y);
check(p.boolean_selector_statement(x,y,b),b ? 1 : 2);
check(p.boolean_guard_statement(x,y,b),b ? 1 : 2);
check(p.boolean_mixed_return(x,y,b),b ? 3 : 7);
check(p.boolean_all_returning(x,y,b),b ? 3 : 4);
}
for (long x : lvalues) for (boolean b : new boolean[]{false,true}) { long y = 4294967297L;
p.long_formal(x,y,b); checks++;
p.long_literal(x,y,b); checks++;
p.long_ascription(x,y,b); checks++;
p.long_if_unit(x,y,b); checks++;
p.long_match_unit(x,y,b); checks++;
p.long_return_value(x,y,b); checks++;
p.long_block_unit(x,y,b); checks++;
p.long_mixed_unit(x,y,b); checks++;
p.long_nested_unit(x,y,b); checks++;
check(p.long_statement(x,y,b),7);
check(p.long_local_read(x,y,b),7);
check(p.long_if_no_else(x,y,b),b ? 1 : 0);
check(p.long_while_body(x,y,b),2);
check(p.long_effect_order(x,y,b),12);
check(p.long_match_statement(x,y,b),b ? 11 : 22);
check(p.long_preserve_result(x,y,b),y);
check(p.long_selector_statement(x,y,b),b ? 1 : 2);
check(p.long_guard_statement(x,y,b),b ? 1 : 2);
check(p.long_mixed_return(x,y,b),b ? 3 : 7);
check(p.long_all_returning(x,y,b),b ? 3 : 4);
}
for (float x : fvalues) for (boolean b : new boolean[]{false,true}) { float y = -0.0f;
p.float_formal(x,y,b); checks++;
p.float_literal(x,y,b); checks++;
p.float_ascription(x,y,b); checks++;
p.float_if_unit(x,y,b); checks++;
p.float_match_unit(x,y,b); checks++;
p.float_return_value(x,y,b); checks++;
p.float_block_unit(x,y,b); checks++;
p.float_mixed_unit(x,y,b); checks++;
p.float_nested_unit(x,y,b); checks++;
check(p.float_statement(x,y,b),7);
check(p.float_local_read(x,y,b),7);
check(p.float_if_no_else(x,y,b),b ? 1 : 0);
check(p.float_while_body(x,y,b),2);
check(p.float_effect_order(x,y,b),12);
check(p.float_match_statement(x,y,b),b ? 11 : 22);
check(p.float_preserve_result(x,y,b),y);
check(p.float_selector_statement(x,y,b),b ? 1 : 2);
check(p.float_guard_statement(x,y,b),b ? 1 : 2);
check(p.float_mixed_return(x,y,b),b ? 3 : 7);
check(p.float_all_returning(x,y,b),b ? 3 : 4);
}
for (double x : dvalues) for (boolean b : new boolean[]{false,true}) { double y = -0.0d;
p.double_formal(x,y,b); checks++;
p.double_literal(x,y,b); checks++;
p.double_ascription(x,y,b); checks++;
p.double_if_unit(x,y,b); checks++;
p.double_match_unit(x,y,b); checks++;
p.double_return_value(x,y,b); checks++;
p.double_block_unit(x,y,b); checks++;
p.double_mixed_unit(x,y,b); checks++;
p.double_nested_unit(x,y,b); checks++;
check(p.double_statement(x,y,b),7);
check(p.double_local_read(x,y,b),7);
check(p.double_if_no_else(x,y,b),b ? 1 : 0);
check(p.double_while_body(x,y,b),2);
check(p.double_effect_order(x,y,b),12);
check(p.double_match_statement(x,y,b),b ? 11 : 22);
check(p.double_preserve_result(x,y,b),y);
check(p.double_selector_statement(x,y,b),b ? 1 : 2);
check(p.double_guard_statement(x,y,b),b ? 1 : 2);
check(p.double_mixed_return(x,y,b),b ? 3 : 7);
check(p.double_all_returning(x,y,b),b ? 3 : 4);
}
p.divide_unit(2); checks++;
throwsArithmetic(() -> p.divide_unit(0));
check(p.divide_statement(2),7);
throwsArithmetic(() -> p.divide_statement(0));
check(p.divide_if(0,false),7);
throwsArithmetic(() -> p.divide_if(0,true));
check(p.divide_after_return(0,true),3);
throwsArithmetic(() -> p.divide_after_return(0,false));
System.out.println("OK "+checks+" checks");
}
}
