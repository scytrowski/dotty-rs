package example;

import java.lang.reflect.Field;

/** Independent Java client for the original Rust-produced paired class batch. */
public final class RustRuntimeProbe {
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
            throw new AssertionError("Pair must retain distinct original Counter references");
        }
        if (pair.leftValue() != firstSeed || pair.rightValue() != nextValue) {
            throw new AssertionError("unexpected values " + pair.leftValue() + "/" + pair.rightValue());
        }

        first.set(firstSeed + 101);
        if (pair.leftValue() != firstSeed + 101 || pair.rightValue() != nextValue) {
            throw new AssertionError("Pair.left must expose the original first reference");
        }
        second.set(secondSeed - 103);
        if (pair.leftValue() != firstSeed + 101 || pair.rightValue() != secondSeed - 103) {
            throw new AssertionError("Pair.right must expose the original second reference");
        }
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        check(7, 3, 42);
        check(-9, 11, 42);
        check(5, 5, -13);
        System.out.println("rust mutable rebinding: signed/equal seeds, original identities and independent mutation passed");
    }
}
