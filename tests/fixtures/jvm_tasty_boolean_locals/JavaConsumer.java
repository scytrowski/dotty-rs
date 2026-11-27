public class JavaConsumer { public static void main(String[] args) {
  example.BooleanLocals p = new example.BooleanLocals();
  assert p.literal();
  for (boolean x : new boolean[]{false,true}) {
    assert p.declared(x) == x;
    assert p.inferred(x) == x;
    assert p.mutable(x) == !x;
    assert p.conditional(x) == !x;
    assert p.loop(x) == (x ? 1 : 0);
    assert p.fromMatch(x) == !x;
    assert p.selector(x) == (x ? 1 : 2);
    assert p.binder(x) == x;
    assert p.effects(x) == x;
    assert p.guardEffects(x,1) == (x ? 30 : 10);
    assert p.guardEffects(x,2) == (x ? 20 : 30);
    assert p.longResult(x,9000000000L) == (x ? 9000000000L : 0L);
    assert p.doubleResult(x,-2.25) == (x ? -2.25 : 0.0);
    assert p.returnFlag(x) == x;
    for (boolean y : new boolean[]{false,true}) {
      assert p.shortInit(x,y) == (x && y);
      assert p.branchAssign(x,y) == !y;
    }
  }
  assert !p.comparison(0) && p.comparison(1) && !p.comparison(-1);
  for (boolean x : new boolean[]{false,true}) {
    assert p.tailFormal(x,false) == x;
    assert p.tailLocal(x,false) == x;
    assert p.tailPattern(x);
  }
  for (boolean x : new boolean[]{false,true}) {
    assert p.effectsOr(x) == !x;
    assert p.assignMatch(x) == !x;
    assert p.loopReturn(x) == (x ? 1 : 0);
    assert p.tailSingleton(x) == !x;
    assert p.siblingScopes(x) == (x ? 1 : 4);
    assert !p.parameterShadow(x);
    assert p.ascribed(x) == x;
    assert p.bothReturn(x,false) == x;
    assert p.floatResult(x,-1.25f) == (x ? -1.25f : 0.0f);
    assert Float.floatToRawIntBits(p.floatResult(x,-0.0f)) == (x ? 0x80000000 : 0);
    assert Double.doubleToRawLongBits(p.doubleResult(x,-0.0)) == (x ? 0x8000000000000000L : 0L);
    p.unit(x);
  }
  assert !p.tailConstant();
  assert p.constantLoop() == 1;
  assert p.falseLoop() == 2;
  System.out.println("boolean local oracle ok");
}}
