package example;

import java.lang.reflect.Field;

public final class PairFactoryTwoLocalIdentityHarness {
    private static Counter field(Pair pair, String name) throws ReflectiveOperationException {
        Field field = Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (Counter) field.get(pair);
    }

    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void check(Pair pair, int left, int right) {
        require(pair.leftValue() == left, "left value: expected " + left + " got " + pair.leftValue());
        require(pair.rightValue() == right, "right value: expected " + right + " got " + pair.rightValue());
    }

    public static void main(String[] args) throws Exception {
        String mode = args[0];
        PairFactory factory = new PairFactory();
        boolean reversed = mode.equals("reversed");
        boolean alias = mode.equals("alias");
        Pair pair = factory.create(7, 3);
        Counter left = field(pair, "left");
        Counter right = field(pair, "right");
        if (alias) {
            require(left == right, "repeated first local must alias both Pair fields");
            require(left.get() == 7, "repeated first local keeps first seed");
            left.set(100);
            check(pair, 100, 100);
            require(right.get() == 100, "mutation through alias visible through right");
        } else {
            require(left != right, "separately initialized locals must be distinct");
            check(pair, reversed ? 3 : 7, reversed ? 7 : 3);
            left.set(100);
            require(pair.leftValue() == 100 && pair.rightValue() == (reversed ? 7 : 3),
                    "left mutation is isolated");
            right.set(-5);
            require(pair.leftValue() == 100 && pair.rightValue() == -5,
                    "right mutation is isolated");
        }

        Pair repeated = factory.create(7, 3);
        Counter repeatedLeft = field(repeated, "left");
        Counter repeatedRight = field(repeated, "right");
        if (alias) {
            require(repeatedLeft == repeatedRight, "repeated call keeps deliberate alias");
            require(repeatedLeft != left, "repeated call allocates a fresh graph");
            check(repeated, 7, 7);
        } else {
            require(repeatedLeft != left && repeatedLeft != right
                    && repeatedRight != left && repeatedRight != right,
                    "repeated call allocates two fresh Counters");
            require(repeatedLeft != repeatedRight, "repeated call Counters remain distinct");
            check(repeated, reversed ? 3 : 7, reversed ? 7 : 3);
        }
        check(factory.create(3, 7), alias ? 3 : (reversed ? 7 : 3), alias ? 3 : (reversed ? 3 : 7));
        check(factory.create(-9, 42), alias ? -9 : (reversed ? 42 : -9), alias ? -9 : (reversed ? -9 : 42));
        System.out.println(mode + "=identity:true;mutations=isolated;repeat=fresh;seeds=7/3,3/7,-9/42");
    }
}
