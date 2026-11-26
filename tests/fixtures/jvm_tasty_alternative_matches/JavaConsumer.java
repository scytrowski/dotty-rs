public final class JavaConsumer {
 public static void main(String[] args) {
  example.AlternativeMatches v=new example.AlternativeMatches();
  for(int x:new int[]{Integer.MIN_VALUE,-1,0,1,2,3,Integer.MAX_VALUE}) {
   if(v.choose(x)!=((x==Integer.MIN_VALUE || x==0 || x==Integer.MAX_VALUE)?10:20)) throw new AssertionError("boundary "+x);
   if(v.nested(x)!=((x>=0 && x<=2)?10:20)) throw new AssertionError("nested "+x);
   if(v.returned(x)!=((x==0 || x==1)?10:20)) throw new AssertionError("return "+x);
   if(v.flag(x)!=(x==0 || x==1)) throw new AssertionError("boolean "+x);
   v.unit(x);
   if(v.bound(x)!=(x<0?x:(x<=1?10:20))) throw new AssertionError("case binder "+x);
   if(v.local(x)!=((x==0 || x==1)?10:20)) throw new AssertionError("local "+x);
   if(v.nestedMatch(x)!=((x==0 || x==1)?10:30)) throw new AssertionError("nested match "+x);
   for(long a:new long[]{Long.MIN_VALUE,-1L,0L,9000000000L,Long.MAX_VALUE}) {
    long z=(x==0 || x==1)?a:Long.MAX_VALUE;
    if(v.longValue(x,a,Long.MAX_VALUE)!=z || v.longReturn(x,a,Long.MAX_VALUE)!=z || v.mixed(x,a,Long.MAX_VALUE)!=z) throw new AssertionError("long "+x);
   }
  }
  for(int x:new int[]{0,1,2}) {
   example.AlternativeMatches t=new example.AlternativeMatches();
   int result=t.guarded(x);
   if(result!=(x<=1?30:20) || t.observed()!=(x==0?12:1)) throw new AssertionError("guard once / order: "+x+" "+t.observed());
  }
  for(int n:new int[]{-1,0,1,2,3,5}) if(v.inLoop(n)!=(n<=0?0:Math.min(n,2))) throw new AssertionError("loop "+n);
  example.AlternativeMatches hit=new example.AlternativeMatches();
  if(hit.guardedTrue(0)!=10 || hit.observed()!=12) throw new AssertionError("true guard");
  System.out.println("alternative match oracle ok");
 }
}
