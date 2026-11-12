package example;

import java.lang.reflect.Field;

public final class RuntimeProbe {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static void check(
            boolean chooseFirst,
            int expectedLeft,
            int expectedRight) throws ReflectiveOperationException {
        Pair pair = new PairFactory().create(7, 3, chooseFirst, 42);
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        if (left == right) {
            throw new AssertionError("Pair fields must retain distinct Counter objects");
        }
        if (pair.leftValue() != expectedLeft || pair.rightValue() != expectedRight) {
            throw new AssertionError(
                    "chooseFirst=" + chooseFirst + " produced " +
                    pair.leftValue() + "/" + pair.rightValue());
        }
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        if (args.length == 1 && args[0].equals("swapped")) {
            check(true, 7, 42);
            check(false, 42, 3);
            System.out.println("swapped branches true=7/42 false=42/3 distinct=true");
        } else {
            check(true, 42, 3);
            check(false, 7, 42);
            System.out.println("conditional selection true=42/3 false=7/42 distinct=true");
        }
    }
}
