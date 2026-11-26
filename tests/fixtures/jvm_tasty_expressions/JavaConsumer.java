public final class JavaConsumer {
  public static void main(String[] args) {
    example.Expressions value = new example.Expressions();
    int[] inputs = {Integer.MIN_VALUE, -257, -1, 0, 1, 2, 255, 256, 257, Integer.MAX_VALUE};
    for (int x : inputs) {
      if (value.intValue(x) != x || value.mask(x) != (x & 255)) throw new AssertionError("Int " + x);
      if (value.classify(x) != (x == 0 ? 10 : x == 1 ? 20 : 30)) throw new AssertionError("match " + x);
      if (value.flag(x) != (x != 0)) throw new AssertionError("Boolean match " + x);
      int expected = (x & 255) == 0 ? 10 : (x & 255) == 1 ? (x == 1 ? 20 : 30) : 40;
      if (value.composed(x) != expected) throw new AssertionError("composed " + x);
      for (int y : inputs) {
        if (value.equal(x, y) != (x == y) || value.different(x, y) != (x != y)) throw new AssertionError("equality");
      }
    }
    if (value.booleanValue(false) || !value.booleanValue(true)) throw new AssertionError("Boolean ascription");
    value.unitValue();
    if (value.read() != 0) throw new AssertionError("Unit ascription");
    value.touch(0);
    value.touch(5);
    if (value.read() != 3) throw new AssertionError("Unit match");
    example.Expressions selector = new example.Expressions();
    if (selector.once() != 11 || selector.read() != 1) throw new AssertionError("selector evaluated once");
    if (selector.once() != 22 || selector.read() != 2) throw new AssertionError("wildcard selector evaluated once");
    System.out.println("expression parity ok");
  }
}
