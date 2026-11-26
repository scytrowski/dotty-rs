import example.LongValues;
public class JavaConsumer {
  public static void main(String[] args) {
    LongValues value = new LongValues();
    assert value.literal() == 9000000000L;
    assert value.minimum() == Long.MIN_VALUE;
    assert value.maximum() == Long.MAX_VALUE;
    assert value.hexadecimal() == -1L;
    assert value.binary() == 42L;
    for (long a : new long[]{Long.MIN_VALUE, -9000000000L, -1L, 0L, 1L, 2147483648L, Long.MAX_VALUE}) {
      long b = ~a;
      assert value.direct(a) == a;
      assert value.mixed(7, a, true, b) == a;
      assert value.mixed(7, a, false, b) == b;
      assert value.intResult(a) == 7;
      assert value.saved(a) == a;
      assert value.inferred(a) == a;
      assert value.assigned(a, b) == b;
      assert value.rotate(0, a, b) == a;
      assert value.rotate(2, a, b) == b;
      for (int tag : new int[]{Integer.MIN_VALUE, -1, 0, 1, Integer.MAX_VALUE}) {
        assert value.choose(tag, a, b) == (tag == 0 ? a : b);
        assert value.guarded(tag, a, b) == (tag < 0 ? a : b);
        assert value.constants(tag) == (tag == 0 ? 1L : 9000000000L);
        assert value.returned(tag, a, b) == (tag == 0 ? a : b);
        assert value.allReturn(tag, a, b) == (tag == 0 ? a : b);
        assert value.deadMatch(tag, a, b) == (tag == 0 ? a : b);
      }
      assert value.early(true, a, b) == a;
      assert value.early(false, a, b) == b;
      assert value.both(true, a, b) == a;
      assert value.both(false, a, b) == b;
      assert value.deadIf(true, a, b) == a;
      assert value.deadIf(false, a, b) == b;
      assert value.inLoop(0, a, b) == a;
      assert value.inLoop(2, a, b) == b;
      assert value.inLoop(4, a, b) == a;
    }
    assert value.once(1L, 9000000000L) == 9000000000L;
    assert value.observed() == 12;
    System.out.println("long value oracle ok");
  }
}
