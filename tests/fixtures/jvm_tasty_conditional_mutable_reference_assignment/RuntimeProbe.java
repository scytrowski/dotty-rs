package example;

import java.lang.reflect.Field;

public final class RuntimeProbe {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static void check(
            int firstSeed,
            int secondSeed,
            boolean chooseSecond,
            int nextValue,
            int expectedLeft,
            int expectedRight) throws ReflectiveOperationException {
        Pair pair = new PairFactory().create(firstSeed, secondSeed, chooseSecond, nextValue);
        Counter first = field(pair, "left");
        Counter second = field(pair, "right");
        if (first == second) {
            throw new AssertionError("Pair must preserve the two distinct allocated Counters");
        }
        if (pair.leftValue() != expectedLeft || pair.rightValue() != expectedRight) {
            throw new AssertionError(
                    "chooseSecond=" + chooseSecond + " produced " +
                    pair.leftValue() + "/" + pair.rightValue());
        }
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        check(7, 3, true, 42, 7, 42);
        check(7, 3, false, 42, 42, 3);
        check(-9, 42, true, 11, -9, 11);
        check(-9, 42, false, 11, 11, 42);
        check(7, 7, true, 42, 7, 42);
        check(7, 7, false, 42, 42, 7);
        System.out.println(
                "conditional mutable reference assignment true=7/42,false=42/3," +
                "signed=-9/11,11/42,equal=7/42,42/7 distinct=true");
    }
}
