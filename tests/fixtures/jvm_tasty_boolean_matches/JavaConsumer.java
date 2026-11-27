import example.BooleanMatches;
public class JavaConsumer {
  public static void main(String[] args) {
    BooleanMatches p = new BooleanMatches();
    for (boolean x : new boolean[]{false, true}) {
      p.reset(); assert p.select(x) == (x ? 40 : 30); assert p.observed() == (x ? 12 : 13);
      for (boolean flag : new boolean[]{false, true}) {
        p.reset(); assert p.alternate(x, flag) == (flag ? 10 : 20); assert p.observed() == 2;
        assert p.shadow(x, flag) == x;
        assert p.nested(x, flag) == x; assert p.conjunction(x, flag) == (x && flag);
        assert p.shortCircuit(x, flag) == ((x && flag) ? 1 : 2);
      }
      assert p.bind(x) == x; assert p.inverted(x) == !x; assert p.loop(3, x) == (x ? 3 : 0); assert p.loop(0, x) == 0;
      assert p.longValue(x, Long.MIN_VALUE, Long.MAX_VALUE) == (x ? Long.MIN_VALUE : Long.MAX_VALUE);
      assert Float.floatToRawIntBits(p.floatValue(x, -0.0f, Float.MIN_VALUE)) == (x ? 0x80000000 : 1);
      assert Double.doubleToRawLongBits(p.doubleValue(x, -0.0, Double.MIN_VALUE)) == (x ? 0x8000000000000000L : 1L);
      assert Double.doubleToRawLongBits(p.returned(x, -0.0, Double.MIN_VALUE)) == (x ? 0x8000000000000000L : 1L);
      assert p.mixed(x) == (x ? 1 : 2); p.unitValue(x);
      assert p.nestedInt(x, 0) == (x ? 7 : 9); assert p.nestedInt(x, 3) == (x ? 8 : 9);
    }
    assert p.singleton(); assert !p.falseSingleton();
    System.out.println("boolean match oracle ok");
  }
}
