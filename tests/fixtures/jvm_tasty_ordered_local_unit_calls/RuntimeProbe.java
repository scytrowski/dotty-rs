package example;

import java.lang.reflect.Field;

public final class RuntimeProbe {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static Pair verify(PairFactory factory, int firstSeed, int secondSeed,
            int expectedLeft, int expectedRight) throws ReflectiveOperationException {
        Pair pair = factory.create(firstSeed, secondSeed);
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        if (left == right || pair.leftValue() != expectedLeft || pair.rightValue() != expectedRight) {
            throw new AssertionError("unexpected pair for seeds " + firstSeed + "/" + secondSeed
                    + ": " + pair.leftValue() + "/" + pair.rightValue());
        }
        return pair;
    }

    public static void main(String[] args) throws ReflectiveOperationException {
        boolean sameReceiver = args.length > 0 && args[0].equals("same-receiver");
        PairFactory factory = new PairFactory();
        Pair first = verify(factory, 7, 3, sameReceiver ? 7 : 3, sameReceiver ? 3 : 7);
        Pair second = verify(factory, -9, 42, sameReceiver ? -9 : 42, sameReceiver ? 42 : -9);
        Pair equal = verify(factory, 7, 7, 7, 7);
        Pair zero = verify(factory, 0, -9, sameReceiver ? 0 : -9, sameReceiver ? -9 : 0);

        if (first == second || first == equal || first == zero || second == equal || second == zero
                || equal == zero) {
            throw new AssertionError("repeated create reused a Pair");
        }
        Pair[] pairs = {first, second, equal, zero};
        for (int i = 0; i < pairs.length; i++) {
            for (int j = i + 1; j < pairs.length; j++) {
                Counter leftI = field(pairs[i], "left");
                Counter rightI = field(pairs[i], "right");
                Counter leftJ = field(pairs[j], "left");
                Counter rightJ = field(pairs[j], "right");
                if (leftI == leftJ || leftI == rightJ || rightI == leftJ || rightI == rightJ) {
                    throw new AssertionError("repeated create reused a Counter");
                }
            }
        }

        Counter left = field(first, "left");
        Counter right = field(first, "right");
        left.set(31);
        if (first.leftValue() != 31 || first.rightValue() != (sameReceiver ? 3 : 7)) {
            throw new AssertionError("left mutation affected the wrong Counter");
        }
        right.set(-8);
        if (first.leftValue() != 31 || first.rightValue() != -8) {
            throw new AssertionError("right mutation affected the wrong Counter");
        }
        if (second.leftValue() != (sameReceiver ? -9 : 42)
                || second.rightValue() != (sameReceiver ? 42 : -9)) {
            throw new AssertionError("mutation escaped into a separate Pair");
        }

        Pair repeated = verify(factory, 7, 3, sameReceiver ? 7 : 3, sameReceiver ? 3 : 7);
        if (repeated == first || field(repeated, "left") == left || field(repeated, "right") == right) {
            throw new AssertionError("later create reused a mutated Pair or Counter");
        }
        System.out.println("two-call-runtime=positive-cases;identity=distinct;mutation=independent;mode="
                + (sameReceiver ? "same-receiver" : "cross-receiver"));
    }
}
