public final class JavaConsumer {
 public static void main(String[] args) {
  example.GuardedMatches v=new example.GuardedMatches();
  if(v.run()!=6 || v.observed()!=12) throw new AssertionError("selector/guard order");
  for(int x:new int[]{Integer.MIN_VALUE,-1,0,1,4,Integer.MAX_VALUE}) {
   if(v.bound(x)!=x || v.nested(x)!=x) throw new AssertionError("binding scope "+x);
  }
  System.out.println("guarded match oracle ok");
 }
}
