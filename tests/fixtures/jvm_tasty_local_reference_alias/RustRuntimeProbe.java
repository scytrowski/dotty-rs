package example;

import java.lang.reflect.Field;

public final class RustRuntimeProbe {
  private static Counter field(Pair pair, String name) throws Exception {
    Field field = Pair.class.getDeclaredField(name);
    field.setAccessible(true);
    return (Counter) field.get(pair);
  }

  private static Pair verify(PairFactory factory, int firstSeed, int secondSeed,
      int expectedLeft, int expectedRight) {
    Pair pair = factory.create(firstSeed, secondSeed);
    Counter left = uncheckedField(pair, "left");
    Counter right = uncheckedField(pair, "right");
    if (left == right || pair.leftValue() != expectedLeft || pair.rightValue() != expectedRight)
      throw new AssertionError("unexpected pair for seeds " + firstSeed + "/" + secondSeed);
    return pair;
  }

  private static Counter uncheckedField(Pair pair, String name) {
    try {
      return field(pair, name);
    } catch (Exception error) {
      throw new AssertionError(error);
    }
  }

  public static void main(String[] args) {
    PairFactory factory = new PairFactory();
    Pair first = verify(factory, 7, 3, 3, 7);
    Counter firstLeft = uncheckedField(first, "left");
    Counter firstRight = uncheckedField(first, "right");
    firstLeft.set(31);
    if (first.leftValue() != 31 || first.rightValue() != 7)
      throw new AssertionError("left mutation changed the wrong Counter");
    firstRight.set(-8);
    if (first.leftValue() != 31 || first.rightValue() != -8)
      throw new AssertionError("right mutation changed the wrong Counter");

    Pair second = verify(factory, -9, 42, 42, -9);
    Pair equal = verify(factory, 7, 7, 7, 7);
    if (first == second || first == equal || second == equal)
      throw new AssertionError("separate factory calls reused a Pair");
    if (uncheckedField(equal, "left") == uncheckedField(equal, "right"))
      throw new AssertionError("equal seed values collapsed object identity");
    if (uncheckedField(first, "left") == uncheckedField(second, "left")
        || uncheckedField(first, "right") == uncheckedField(second, "right"))
      throw new AssertionError("separate factory calls reused a Counter");

    Pair fromSecondFactory = verify(new PairFactory(), 7, 7, 7, 7);
    if (uncheckedField(equal, "left") == uncheckedField(fromSecondFactory, "left")
        || uncheckedField(equal, "right") == uncheckedField(fromSecondFactory, "right"))
      throw new AssertionError("separate factory instances reused a Counter");
    System.out.println("alias-runtime=3/7,42/-9,7/7;identity=distinct;mutations=independent;repeat=fresh");
  }
}
