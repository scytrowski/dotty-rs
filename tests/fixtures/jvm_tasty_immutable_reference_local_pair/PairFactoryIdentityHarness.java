package example;

import java.lang.reflect.Field;
import java.lang.reflect.Method;

public final class PairFactoryIdentityHarness {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void check(Pair pair, Counter existing, boolean reversed, int seed)
            throws ReflectiveOperationException {
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        Counter existingSide = reversed ? right : left;
        Counter freshSide = reversed ? left : right;
        require(existingSide == existing, "caller Counter identity moved to the wrong Pair field");
        require(freshSide != existing, "fresh local aliases the caller Counter");
        require(freshSide.get() == seed, "fresh local lost the constructor seed");
    }

    public static void main(String[] args) throws Exception {
        boolean reversed = args.length == 1 && args[0].equals("reversed");
        PairFactory factory = new PairFactory();
        Counter existing = new Counter(7);
        Method create = PairFactory.class.getMethod(
                reversed ? "createReversed" : "create", Counter.class, int.class);
        Pair first = (Pair) create.invoke(factory, existing, 3);
        check(first, existing, reversed, 3);
        require(first.leftValue() == (reversed ? 3 : 7), "initial left value");
        require(first.rightValue() == (reversed ? 7 : 3), "initial right value");

        existing.set(42);
        require(first.leftValue() == (reversed ? 3 : 42), "existing mutation identity");
        require(first.rightValue() == (reversed ? 42 : 3), "existing mutation isolation");
        Counter fresh = reversed ? field(first, "left") : field(first, "right");
        fresh.set(-9);
        require(first.leftValue() == (reversed ? -9 : 42), "fresh mutation isolation");
        require(first.rightValue() == (reversed ? 42 : -9), "fresh mutation identity");

        Pair repeated = (Pair) create.invoke(factory, existing, 11);
        check(repeated, existing, reversed, 11);
        Counter repeatedFresh = reversed ? field(repeated, "left") : field(repeated, "right");
        require(repeatedFresh != fresh, "repeated call reused the previous fresh Counter");
        require((reversed ? field(repeated, "right") : field(repeated, "left")) == existing,
                "repeated call lost caller identity");
        require(fresh.get() == -9, "repeated call mutated the previous fresh Counter");

        Pair zero = (Pair) create.invoke(factory, existing, 0);
        Pair negative = (Pair) create.invoke(factory, existing, -5);
        check(zero, existing, reversed, 0);
        check(negative, existing, reversed, -5);
        require((reversed ? field(zero, "left") : field(zero, "right")) != repeatedFresh,
                "zero seed reused the previous fresh Counter");
        require((reversed ? field(negative, "left") : field(negative, "right"))
                        != (reversed ? field(zero, "left") : field(zero, "right")),
                "negative seed reused the zero-seed Counter");

        System.out.println((reversed ? "reversed" : "primary")
                + "=identity:true;mutations=isolated;repeat=fresh;seeds=0,-5");
    }
}
