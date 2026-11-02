import java.lang.reflect.Field;

public final class PairFactoryForwardingHarness {
    public static void main(String[] args) throws Exception {
        example.Counter a = new example.Counter(7);
        example.Counter b = new example.Counter(3);
        example.PairFactory firstFactory = new example.PairFactory();
        example.PairFactory secondFactory = new example.PairFactory();
        example.Pair pair = firstFactory.wrap(a, b);
        identity(pair, "left", a); identity(pair, "right", b);
        expect(pair.leftValue(), 7); expect(pair.rightValue(), 3);
        a.set(42); expect(pair.leftValue(), 42); expect(pair.rightValue(), 3);
        b.set(-9); expect(pair.leftValue(), 42); expect(pair.rightValue(), -9);

        example.Pair reverse = secondFactory.wrap(b, a);
        identity(reverse, "left", b); identity(reverse, "right", a);
        expect(reverse.leftValue(), -9); expect(reverse.rightValue(), 42);
        example.Pair repeated = new example.PairFactory().wrap(a, b);
        identity(repeated, "left", a); identity(repeated, "right", b);
        if (repeated == pair || firstFactory == secondFactory)
            throw new AssertionError("separate calls and factory instances remain independent");

        example.Pair alias = secondFactory.wrap(a, a);
        identity(alias, "left", a); identity(alias, "right", a);
        a.set(100); expect(alias.leftValue(), 100); expect(alias.rightValue(), 100);
        expect(pair.leftValue(), 100); expect(pair.rightValue(), -9);
        expect(reverse.leftValue(), -9); expect(reverse.rightValue(), 100);
        System.out.println("forward=7/3,42/3,42/-9;reverse=-9/42;repeat=true;alias=100/100");
    }
    private static void identity(example.Pair pair, String fieldName, example.Counter expected)
        throws Exception {
        Field field = example.Pair.class.getDeclaredField(fieldName);
        field.setAccessible(true);
        if (field.get(pair) != expected)
            throw new AssertionError(fieldName + " must preserve the original argument object");
    }
    private static void expect(int actual, int expected) {
        if (actual != expected) throw new AssertionError("expected " + expected + ", got " + actual);
    }
}
