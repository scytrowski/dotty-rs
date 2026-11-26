public final class JavaConsumer {
 public static void main(String[] args) {
  example.EarlyReturns v=new example.EarlyReturns();
  for(int x:new int[]{Integer.MIN_VALUE,-1,0,1,3,5,Integer.MAX_VALUE}) {
   if(v.direct(x)!=x || v.both(x)!=(x<0 ? -1 : x+1) || v.matched(x)!=(x<0 ? x : x+1)) throw new AssertionError("return value");
   if(v.positive(x)!=(x>=0)) throw new AssertionError("Boolean exit");
  }
  for(int x:new int[]{-1,0,1,2,3,5}) if(v.inLoop(x)!=(x>=3 ? 3 : 0)) throw new AssertionError("loop exit");
  for(boolean b:new boolean[]{false,true}) if(v.booleanValue(b)!=b) throw new AssertionError("Boolean return");
  v.unit();
  if(v.choose(-1)!=1 || v.choose(0)!=12 || v.observed()!=2) throw new AssertionError("return expression order");
  v.stop(true);
  if(v.observed()!=2) throw new AssertionError("Unit return");
  v.stop(false);
  if(v.observed()!=102 || v.once()!=103 || v.observed()!=103) throw new AssertionError("unreachable effects");
  if(v.deadIf(true)!=7 || v.deadIf(false)!=3 || v.deadMatch(0)!=7 || v.deadMatch(1)!=3
      || v.deadBoth(true)!=1 || v.deadBoth(false)!=2 || v.observed()!=103) throw new AssertionError("unreachable branch effects");
  System.out.println("local return oracle ok");
 }
}
