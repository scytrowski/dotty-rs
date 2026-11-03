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

    private static Pair invoke(PairFactory factory, Method method, Counter existing, int seed)
            throws ReflectiveOperationException {
        return (Pair) method.invoke(factory, existing, seed);
    }

    private static void checkPair(Pair pair, Counter existing, boolean reversed, int expectedNew)
            throws ReflectiveOperationException {
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        Counter existingSide = reversed ? right : left;
        Counter newSide = reversed ? left : right;
        require(existingSide == existing, "caller Counter identity moved to the wrong Pair field");
        require(newSide != existing, "factory did not allocate a distinct Counter");
        require(newSide.get() == expectedNew, "new Counter did not retain its seed");
    }

    public static void main(String[] args) throws Exception {
        boolean reversed = args.length == 1 && args[0].equals("reversed");
        Counter existing = new Counter(7);
        PairFactory factory = new PairFactory();
        Method method = PairFactory.class.getMethod(
                reversed ? "createAndWrap" : "wrapAndCreate", Counter.class, int.class);

        Pair first = invoke(factory, method, existing, 3);
        checkPair(first, existing, reversed, 3);
        existing.set(42);
        require(first.leftValue() == (reversed ? 3 : 42), "existing mutation did not follow identity");
        require(first.rightValue() == (reversed ? 42 : 3), "existing mutation changed the new Counter");
        Counter firstNew = reversed ? field(first, "left") : field(first, "right");
        firstNew.set(-9);
        require(first.leftValue() == (reversed ? -9 : 42), "new Counter mutation changed the existing side");
        require(first.rightValue() == (reversed ? 42 : -9), "new Counter mutation changed the existing side");

        Pair repeated = invoke(factory, method, existing, 11);
        checkPair(repeated, existing, reversed, 11);
        Counter repeatedNew = reversed ? field(repeated, "left") : field(repeated, "right");
        require(repeatedNew != firstNew, "next factory call reused the previous new Counter");
        require((reversed ? field(repeated, "right") : field(repeated, "left")) == existing,
                "next factory call lost caller identity");

        Pair zero = invoke(factory, method, existing, 0);
        Pair negative = invoke(factory, method, existing, -5);
        checkPair(zero, existing, reversed, 0);
        checkPair(negative, existing, reversed, -5);
        require((reversed ? field(zero, "left") : field(zero, "right")) != repeatedNew,
                "zero-seed call reused a prior Counter");
        require((reversed ? field(negative, "left") : field(negative, "right"))
                        != (reversed ? field(zero, "left") : field(zero, "right")),
                "negative-seed call reused the zero-seed Counter");

        System.out.println((reversed ? "createAndWrap" : "wrapAndCreate")
                + "=identity:true;first=42/-9;repeat=11;zero=0;negative=-5;fresh:true");
    }
}
