package example;

import java.lang.reflect.Field;

public final class RuntimeProbe {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static void verify(int firstSeed, int secondSeed, int expectedLeft, int expectedRight)
            throws ReflectiveOperationException {
        Pair pair = new PairFactory().create(firstSeed, secondSeed);
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        if (left == right || pair.leftValue() != expectedLeft || pair.rightValue() != expectedRight) {
            throw new AssertionError("unexpected pair for seeds " + firstSeed + "/" + secondSeed);
        }
        System.out.println(pair.leftValue() + "/" + pair.rightValue() + " distinct=" + (left != right));
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        verify(7, 3, 3, 7);
        verify(-9, 42, 42, -9);
    }
}
