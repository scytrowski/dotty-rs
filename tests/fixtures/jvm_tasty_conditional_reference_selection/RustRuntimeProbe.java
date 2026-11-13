package example;

import java.lang.reflect.Field;

public final class RustRuntimeProbe {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static Pair check(
            PairFactory factory,
            int firstSeed,
            int secondSeed,
            boolean chooseFirst,
            int nextValue,
            int expectedLeft,
            int expectedRight) throws ReflectiveOperationException {
        Pair pair = factory.create(firstSeed, secondSeed, chooseFirst, nextValue);
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        if (left == right) {
            throw new AssertionError("Pair must retain two distinct Counter references");
        }
        if (pair.leftValue() != expectedLeft || pair.rightValue() != expectedRight) {
            throw new AssertionError(
                    "create(" + firstSeed + "," + secondSeed + "," + chooseFirst + "," +
                    nextValue + ") expected " + expectedLeft + "/" + expectedRight +
                    " but got " + pair.leftValue() + "/" + pair.rightValue());
        }
        return pair;
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        PairFactory factory = new PairFactory();
        Pair firstChosen = check(factory, 7, 3, true, 42, 42, 3);
        Pair secondChosen = check(factory, 7, 3, false, 42, 7, 42);
        Pair negativeFirst = check(factory, -9, 42, true, 0, 0, 42);
        Pair negativeSecond = check(factory, -9, 42, false, 0, -9, 0);
        Pair equalSeeds = check(factory, 7, 7, true, 42, 42, 7);

        Pair[] results = {firstChosen, secondChosen, negativeFirst, negativeSecond, equalSeeds};
        for (int leftIndex = 0; leftIndex < results.length; leftIndex++) {
            Counter[] current = {
                    field(results[leftIndex], "left"),
                    field(results[leftIndex], "right")
            };
            for (int rightIndex = leftIndex + 1; rightIndex < results.length; rightIndex++) {
                Counter[] next = {
                        field(results[rightIndex], "left"),
                        field(results[rightIndex], "right")
                };
                for (Counter first : current) {
                    for (Counter second : next) {
                        if (first == second) {
                            throw new AssertionError("each call must allocate two fresh Counters");
                        }
                    }
                }
            }
        }

        Counter originalLeft = field(firstChosen, "left");
        Counter originalRight = field(firstChosen, "right");
        originalLeft.set(31);
        if (firstChosen.leftValue() != 31 || firstChosen.rightValue() != 3) {
            throw new AssertionError("Pair left field is not the original first Counter");
        }
        originalRight.set(-8);
        if (firstChosen.leftValue() != 31 || firstChosen.rightValue() != -8) {
            throw new AssertionError("Pair right field is not the original second Counter");
        }

        System.out.println(
                "conditional-rust-runtime=true:42/3,false:7/42,negative:0/42,-9/0," +
                "equal:42/7,identity=original-distinct,fresh=true");
    }
}
