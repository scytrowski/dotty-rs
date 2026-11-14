package example;

import java.lang.reflect.Field;

public final class RuntimeProbe {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static void check(int firstSeed, int secondSeed, int nextValue)
            throws ReflectiveOperationException {
        Pair pair = new PairFactory().create(firstSeed, secondSeed, nextValue);
        Counter first = field(pair, "left");
        Counter second = field(pair, "right");
        if (first == second) {
            throw new AssertionError("Pair must retain two distinct Counter allocations");
        }
        if (pair.leftValue() != firstSeed || pair.rightValue() != nextValue) {
            throw new AssertionError("unexpected values " + pair.leftValue() + "/" + pair.rightValue());
        }
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        check(7, 3, 42);
        check(-9, 42, 11);
        check(5, 5, -13);
        System.out.println("mutable reference rebinding values=7/42,-9/11,5/-13 distinct=true");
    }
}
