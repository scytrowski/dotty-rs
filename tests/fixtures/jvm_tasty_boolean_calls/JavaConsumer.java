import example.BooleanCalls;
public class JavaConsumer {
    static int checks;
    static void check(boolean actual, boolean expected) {
        if (actual != expected) throw new AssertionError("boolean " + checks);
        checks++;
    }
    static void check(int actual, int expected) {
        if (actual != expected) throw new AssertionError("int " + checks);
        checks++;
    }
    static void throwsArithmetic(Runnable f) {
        try { f.run(); } catch (ArithmeticException e) { checks++; return; }
        throw new AssertionError("missing exception " + checks);
    }
    public static void main(String[] args) {
        var p = new BooleanCalls();
        check(p.noarg(), true);
        for (int x : new int[] {-2, 0, 3}) {
            boolean positive = x > 0;
            check(p.direct(x), positive);
            check(p.explicit(x), positive);
            check(p.nested(x), positive);
            check(p.not(x), !positive);
            check(p.local(x), positive);
            check(p.returns(x), positive);
            check(p.condition(x), positive ? 1 : 2);
            check(p.selector(x), positive ? 1 : 2);
            check(p.guard(x), positive ? 1 : 2);
            check(p.intCall(x), positive ? 1 : 2);
            check(p.loop(x), x > 0 ? 0 : x);
            for (boolean b : new boolean[] {false, true}) {
                check(p.mixed(x, b), b && positive);
                check(p.mixedCall(x, b), b && positive);
            }
        }
        for (boolean a : new boolean[] {false, true}) {
            for (boolean b : new boolean[] {false, true}) {
                check(p.both(a, b), a && b);
                check(p.ordered(a, b), 12);
                check(p.eagerState(a, b), 12);
                check(p.andState(a, b), a ? 12 : 1);
                check(p.orState(a, b), a ? 1 : 12);
                check(p.conditionalArg(a, b), a ? b : !b);
            }
            throwsArithmetic(() -> p.eager(a, 0));
            if (a) throwsArithmetic(() -> p.shortAnd(a, 0));
            else check(p.shortAnd(a, 0), false);
            if (!a) throwsArithmetic(() -> p.shortOr(a, 0));
            else check(p.shortOr(a, 0), true);
        }
        throwsArithmetic(() -> p.discard(0));
        p.discard(2); checks++;
        System.out.println("OK " + checks + " checks");
    }
}
