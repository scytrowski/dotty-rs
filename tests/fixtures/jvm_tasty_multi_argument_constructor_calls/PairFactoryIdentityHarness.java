import java.lang.reflect.Field;

public final class PairFactoryIdentityHarness {
    public static void main(String[] args) throws ReflectiveOperationException {
        example.PairFactory factory = new example.PairFactory();
        example.Pair first = factory.create(7, 3);
        example.Pair reversed = factory.create(3, 7);
        example.Pair signed = factory.create(-9, 42);
        if (first == reversed || first == signed || reversed == signed) {
            throw new AssertionError("each create must return an independent Pair");
        }
        example.Counter[][] counters = {
            {field(first, "left"), field(first, "right")},
            {field(reversed, "left"), field(reversed, "right")},
            {field(signed, "left"), field(signed, "right")}
        };
        for (int i = 0; i < counters.length; i++) {
            if (counters[i][0] == counters[i][1]) {
                throw new AssertionError("left and right must be distinct Counters");
            }
            for (int j = i + 1; j < counters.length; j++) {
                for (int left = 0; left < 2; left++) {
                    for (int right = 0; right < 2; right++) {
                        if (counters[i][left] == counters[j][right]) {
                            throw new AssertionError("create calls must not share Counters");
                        }
                    }
                }
            }
        }
        expect(first.leftValue(), 7);
        expect(first.rightValue(), 3);
        expect(reversed.leftValue(), 3);
        expect(reversed.rightValue(), 7);
        expect(signed.leftValue(), -9);
        expect(signed.rightValue(), 42);

        counters[0][0].set(100);
        expect(first.leftValue(), 100);
        expect(first.rightValue(), 3);
        expect(reversed.leftValue(), 3);
        expect(reversed.rightValue(), 7);
        expect(signed.leftValue(), -9);
        counters[0][1].set(-5);
        expect(first.leftValue(), 100);
        expect(first.rightValue(), -5);
        expect(reversed.leftValue(), 3);
        expect(signed.leftValue(), -9);
        counters[1][0].set(-100);
        expect(first.leftValue(), 100);
        expect(first.rightValue(), -5);
        expect(reversed.leftValue(), -100);
        expect(reversed.rightValue(), 7);
        expect(signed.leftValue(), -9);
        expect(signed.rightValue(), 42);
        System.out.println("values=7/3,3/7,-9/42; independent=true; mutations=100/-5,-100/7");
    }

    private static example.Counter field(example.Pair pair, String name)
        throws ReflectiveOperationException {
        Field field = example.Pair.class.getDeclaredField(name);
        field.setAccessible(true);
        return (example.Counter) field.get(pair);
    }

    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
