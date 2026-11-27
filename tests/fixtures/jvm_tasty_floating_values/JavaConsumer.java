final class ValueChecks {
 static void sameF(float a, float b) {
  if(Float.isNaN(b)? !Float.isNaN(a):Float.floatToRawIntBits(a)!=Float.floatToRawIntBits(b)) throw new AssertionError("float bits");
 }
 static void sameD(double a, double b) {
  if(Double.isNaN(b)? !Double.isNaN(a):Double.doubleToRawLongBits(a)!=Double.doubleToRawLongBits(b)) throw new AssertionError("double bits");
 }
 static final float[] FS={0.0f,-0.0f,Float.MIN_VALUE,Float.MAX_VALUE,Float.POSITIVE_INFINITY,Float.NEGATIVE_INFINITY,Float.intBitsToFloat(0x7fc00001)};
 static final double[] DS={0.0,-0.0,Double.MIN_VALUE,Double.MAX_VALUE,Double.POSITIVE_INFINITY,Double.NEGATIVE_INFINITY,Double.longBitsToDouble(0x7ff8000000000001L)};
 static void check(example.FloatingValues p) {
  sameF(p.fzero(),0.0f); sameF(p.fnegativeZero(),-0.0f); sameF(p.fsmall(),Float.MIN_VALUE); sameF(p.flarge(),Float.MAX_VALUE);
  sameD(p.dzero(),0.0); sameD(p.dnegativeZero(),-0.0); sameD(p.dsmall(),Double.MIN_VALUE); sameD(p.dlarge(),Double.MAX_VALUE);
  for(float x:FS) { sameF(p.directF(x),x);sameF(p.savedF(x),x);
   for(boolean flag:new boolean[]{false,true}) {sameF(p.chooseF(flag,x,-0.0f),flag?x:-0.0f);sameF(p.returnedF(flag,x,-0.0f),flag?x:-0.0f);}
   for(int n:new int[]{0,1,2}) sameF(p.rotateF(n,x,-0.0f),n==0?x:-0.0f);
   for(int n:new int[]{-1,0,1,2}) sameF(p.allReturnF(n,x,-0.0f),(n==0||n==1)?x:-0.0f);
  }
  for(double x:DS) { sameD(p.directD(x),x);sameD(p.savedD(x),x);
   for(boolean flag:new boolean[]{false,true}) {sameD(p.chooseD(flag,x,-0.0),flag?x:-0.0);sameD(p.returnedD(flag,x,-0.0),flag?x:-0.0);sameD(p.mixed(7,1.0f,Long.MIN_VALUE,x,flag),flag?x:-0.0);}
   for(int n:new int[]{0,1,2}) sameD(p.rotateD(n,x,-0.0),n==0?x:-0.0);
   for(int n:new int[]{-1,0,1,2}) sameD(p.allReturnD(n,x,-0.0),(n==0||n==1)?x:-0.0);
  }
 }
}

public final class JavaConsumer {
 public static void main(String[] args) {
  example.FloatingValues p=new example.FloatingValues();ValueChecks.check(p);
  for(int n:new int[]{-1,0,1,2}) for(boolean flag:new boolean[]{false,true}) {
   for(float x:ValueChecks.FS) {ValueChecks.sameF(p.matchedF(n,flag,x,-0.0f),((n==0||n==1)&&flag)?x:-0.0f);ValueChecks.sameF(p.deadF(flag,x,-0.0f),flag?x:-0.0f);}
   for(double x:ValueChecks.DS) {ValueChecks.sameD(p.matchedD(n,flag,x,-0.0),((n==0||n==1)&&flag)?x:-0.0);ValueChecks.sameD(p.deadD(flag,x,-0.0),flag?x:-0.0);}
  }
  example.FloatingValues effect=new example.FloatingValues();
  ValueChecks.sameF(effect.guardedF(0,1.5f,-0.0f),-0.0f);
  if(effect.observed()!=12) throw new AssertionError("selector/guard once");
  for(double x:ValueChecks.DS) {
   ValueChecks.sameD(p.boundD(1,x,-0.0),x);
   ValueChecks.sameD(p.boundD(0,x,-0.0),-0.0);
   ValueChecks.sameD(p.inLoopD(3,x,-0.0),x);
   ValueChecks.sameD(p.inLoopD(1,x,-0.0),-0.0);
  }
  System.out.println("floating value oracle ok");
 }
}
