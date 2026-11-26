public final class JavaConsumer {
 public static void main(String[] args) {
  example.GuardedMatches v=new example.GuardedMatches();
  if(v.run()!=6 || v.observed()!=12) throw new AssertionError("selector/guard order");
  for(int x:new int[]{Integer.MIN_VALUE,-1,0,1,4,Integer.MAX_VALUE}) {
   if(v.bound(x)!=x || v.nested(x)!=x) throw new AssertionError("binding scope "+x);
  }
  if(v.constant()!=0 || v.negative(-1)!=-1 || v.negative(1)!=0) throw new AssertionError("literal binding");
  for(int x:new int[]{Integer.MIN_VALUE,-1,0,1,4,Integer.MAX_VALUE}) {
   if(v.shadow(x)!=(x>0 ? x+1 : x)) throw new AssertionError("shadowing");
   for(boolean enabled:new boolean[]{false,true}) if(v.flag(x,enabled)!=(enabled && x>0)) throw new AssertionError("Boolean result");
  }
  example.GuardedMatches effect=new example.GuardedMatches();
  if(effect.shortCircuit(false)!=0 || effect.observed()!=0) throw new AssertionError("short circuit");
  if(effect.shortCircuit(true)!=2 || effect.observed()!=2) throw new AssertionError("guard effect");
  example.GuardedMatches literal=new example.GuardedMatches();
  if(literal.literalFirst(1)!=20 || literal.observed()!=0) throw new AssertionError("unmatched guard ran");
  if(literal.literalFirst(0)!=10 || literal.observed()!=2) throw new AssertionError("false guard continuation");
  literal.touch(-1);
  if(literal.observed()!=2) throw new AssertionError("Unit fallback");
  literal.touch(5);
  if(literal.observed()!=7) throw new AssertionError("Unit effect");
  System.out.println("guarded match oracle ok");
 }
}
