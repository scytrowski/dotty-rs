import example.BooleanExhaustiveMatches;
public final class JavaConsumer {
  static void eq(int a,int b){if(a!=b)throw new AssertionError(a+" != "+b);}
  static void ok(boolean b){if(!b)throw new AssertionError();}
  static void fbits(float a,float b){if(Float.isNaN(b))ok(Float.isNaN(a));else ok(Float.floatToRawIntBits(a)==Float.floatToRawIntBits(b));}
  static void dbits(double a,double b){if(Double.isNaN(b))ok(Double.isNaN(a));else ok(Double.doubleToRawLongBits(a)==Double.doubleToRawLongBits(b));}
  public static void main(String[] args){
    BooleanExhaustiveMatches p=new BooleanExhaustiveMatches();
    for(boolean x:new boolean[]{false,true}){
      eq(p.ordered(x),x?1:2);eq(p.reversed(x),x?1:2);
      eq(p.alternative(x),3);eq(p.nested_alternative(x),3);
      ok(p.bind_literals(x)==x);ok(p.local_bool(x)==!x);
      eq(p.guard_effect(x),x?11:20);eq(p.selector_effect(x),x?11:21);
      eq(p.return_mixed(x),x?1:2);eq(p.return_all(x),x?1:2);
      ok(p.result_boolean(x)==x);p.result_unit(x);p.return_unit(x);
      ok(p.result_long(x)==(x?Long.MAX_VALUE:Long.MIN_VALUE));
      fbits(p.result_float(x),x?-0.0f:Float.MIN_VALUE);dbits(p.result_double(x),x?-0.0:Double.MIN_VALUE);
      ok(p.return_long(x,Long.MIN_VALUE,Long.MAX_VALUE)==(x?Long.MIN_VALUE:Long.MAX_VALUE));
      for(float a:new float[]{-0.0f,Float.MIN_VALUE,Float.MAX_VALUE,Float.POSITIVE_INFINITY,Float.NaN})fbits(p.return_float(x,a,0.0f),x?a:0.0f);
      for(double a:new double[]{-0.0,Double.MIN_VALUE,Double.MAX_VALUE,Double.POSITIVE_INFINITY,Double.NaN})dbits(p.return_double(x,a,0.0),x?a:0.0);
      for(boolean y:new boolean[]{false,true}){
        eq(p.guard_before_complete(x,y),x?(y?3:1):2);
        eq(p.guarded_alternative_before_complete(x,y),x?(y?3:1):2);
        ok(p.return_tail(x,y)==!x);ok(p.return_boolean(x,y,!y)==(x?y:!y));
        eq(p.nested(x,y),x?(y?1:2):3);
      }
    }
    System.out.println("exhaustive boolean match oracle ok");
  }
}
