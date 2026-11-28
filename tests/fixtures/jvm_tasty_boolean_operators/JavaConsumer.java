import example.BooleanOperators;

public class JavaConsumer {
    static int checks;
    static void check(boolean actual, boolean expected) {
        if (actual != expected) throw new AssertionError("boolean check " + checks);
        checks++;
    }
    static void check(int actual, int expected) {
        if (actual != expected) throw new AssertionError("int check " + checks + ": " + actual + " != " + expected);
        checks++;
    }
    static void throwsArithmetic(Runnable f) {
        boolean thrown = false;
        try { f.run(); } catch (ArithmeticException e) { thrown = true; }
        if (!thrown) throw new AssertionError("missing exception " + checks);
        checks++;
    }
    public static void main(String[] args) {
        var p = new BooleanOperators();
        for (boolean a : new boolean[] {false, true}) {
            for (boolean b : new boolean[] {false, true}) {
                int row = (a ? 2 : 0) + (b ? 1 : 0);
                // and: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {false, false, false, true}[row];
                    check(p.and_formals(a, b), value);
                    check(p.and_operands(a, b), value);
                    check(p.and_local(a, b), value);
                    check(p.and_match(a, b), value);
                    check(p.and_if(a, b), value);
                    check(p.and_literals(a, b), false);
                    check(p.and_nested(a, b), !value || (a && b));
                    check(p.and_guard(a, b), value ? 1 : 2);
                    check(p.and_while(a, b), value ? 2 : 0);
                    p.and_discard(a, b); checks++;
                    check(p.and_return(a, b), a ? value : b);
                    check(p.and_order(a, b), 12);
                    check(p.and_discard_order(a, b), 12);
                    if (!b) {
                        throwsArithmetic(() -> p.and_throw(a, 0));
                        check(p.and_throw(a, 2), a);
                    }
                    if (!a) throwsArithmetic(() -> p.and_after_return(a, b, 0));
                    else check(p.and_after_return(a, b, 0), a ? 3 : 7);
                    check(p.and_after_return(a, b, 2), a ? 3 : 7);
                }
                // or: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {false, true, true, true}[row];
                    check(p.or_formals(a, b), value);
                    check(p.or_operands(a, b), value);
                    check(p.or_local(a, b), value);
                    check(p.or_match(a, b), value);
                    check(p.or_if(a, b), value);
                    check(p.or_literals(a, b), true);
                    check(p.or_nested(a, b), !value || (a && b));
                    check(p.or_guard(a, b), value ? 1 : 2);
                    check(p.or_while(a, b), value ? 2 : 0);
                    p.or_discard(a, b); checks++;
                    check(p.or_return(a, b), a ? value : b);
                    check(p.or_order(a, b), 12);
                    check(p.or_discard_order(a, b), 12);
                    if (!b) {
                        throwsArithmetic(() -> p.or_throw(a, 0));
                        check(p.or_throw(a, 2), true);
                    }
                    if (!a) throwsArithmetic(() -> p.or_after_return(a, b, 0));
                    else check(p.or_after_return(a, b, 0), a ? 3 : 7);
                    check(p.or_after_return(a, b, 2), a ? 3 : 7);
                }
                // xor: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {false, true, true, false}[row];
                    check(p.xor_formals(a, b), value);
                    check(p.xor_operands(a, b), value);
                    check(p.xor_local(a, b), value);
                    check(p.xor_match(a, b), value);
                    check(p.xor_if(a, b), value);
                    check(p.xor_literals(a, b), true);
                    check(p.xor_nested(a, b), !value || (a && b));
                    check(p.xor_guard(a, b), value ? 1 : 2);
                    check(p.xor_while(a, b), value ? 2 : 0);
                    p.xor_discard(a, b); checks++;
                    check(p.xor_return(a, b), a ? value : b);
                    check(p.xor_order(a, b), 12);
                    check(p.xor_discard_order(a, b), 12);
                    if (!b) {
                        throwsArithmetic(() -> p.xor_throw(a, 0));
                        check(p.xor_throw(a, 2), !a);
                    }
                    if (!a) throwsArithmetic(() -> p.xor_after_return(a, b, 0));
                    else check(p.xor_after_return(a, b, 0), a ? 3 : 7);
                    check(p.xor_after_return(a, b, 2), a ? 3 : 7);
                }
                // eq: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {true, false, false, true}[row];
                    check(p.eq_formals(a, b), value);
                    check(p.eq_operands(a, b), value);
                    check(p.eq_local(a, b), value);
                    check(p.eq_match(a, b), value);
                    check(p.eq_if(a, b), value);
                    check(p.eq_literals(a, b), false);
                    check(p.eq_nested(a, b), !value || (a && b));
                    check(p.eq_guard(a, b), value ? 1 : 2);
                    check(p.eq_while(a, b), value ? 2 : 0);
                    p.eq_discard(a, b); checks++;
                    check(p.eq_return(a, b), a ? value : b);
                    check(p.eq_order(a, b), 12);
                    check(p.eq_discard_order(a, b), 12);
                    if (!b) {
                        throwsArithmetic(() -> p.eq_throw(a, 0));
                        check(p.eq_throw(a, 2), a);
                    }
                    if (!a) throwsArithmetic(() -> p.eq_after_return(a, b, 0));
                    else check(p.eq_after_return(a, b, 0), a ? 3 : 7);
                    check(p.eq_after_return(a, b, 2), a ? 3 : 7);
                }
                // ne: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {false, true, true, false}[row];
                    check(p.ne_formals(a, b), value);
                    check(p.ne_operands(a, b), value);
                    check(p.ne_local(a, b), value);
                    check(p.ne_match(a, b), value);
                    check(p.ne_if(a, b), value);
                    check(p.ne_literals(a, b), true);
                    check(p.ne_nested(a, b), !value || (a && b));
                    check(p.ne_guard(a, b), value ? 1 : 2);
                    check(p.ne_while(a, b), value ? 2 : 0);
                    p.ne_discard(a, b); checks++;
                    check(p.ne_return(a, b), a ? value : b);
                    check(p.ne_order(a, b), 12);
                    check(p.ne_discard_order(a, b), 12);
                    if (!b) {
                        throwsArithmetic(() -> p.ne_throw(a, 0));
                        check(p.ne_throw(a, 2), !a);
                    }
                    if (!a) throwsArithmetic(() -> p.ne_after_return(a, b, 0));
                    else check(p.ne_after_return(a, b, 0), a ? 3 : 7);
                    check(p.ne_after_return(a, b, 2), a ? 3 : 7);
                }
                // short_and: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {false, false, false, true}[row];
                    check(p.short_and_formals(a, b), value);
                    check(p.short_and_operands(a, b), value);
                    check(p.short_and_local(a, b), value);
                    check(p.short_and_match(a, b), value);
                    check(p.short_and_if(a, b), value);
                    check(p.short_and_literals(a, b), false);
                    check(p.short_and_nested(a, b), !value || (a && b));
                    check(p.short_and_guard(a, b), value ? 1 : 2);
                    check(p.short_and_while(a, b), value ? 2 : 0);
                    p.short_and_discard(a, b); checks++;
                    check(p.short_and_return(a, b), a ? value : b);
                    check(p.short_and_order(a, b), a ? 12 : 1);
                    check(p.short_and_discard_order(a, b), a ? 12 : 1);
                    if (!b) {
                        if (a) throwsArithmetic(() -> p.short_and_throw(a, 0));
                        else check(p.short_and_throw(a, 0), false);
                        check(p.short_and_throw(a, 2), a);
                    }
                    if (!a && b) throwsArithmetic(() -> p.short_and_after_return(a, b, 0));
                    else check(p.short_and_after_return(a, b, 0), a ? 3 : 7);
                    check(p.short_and_after_return(a, b, 2), a ? 3 : 7);
                }
                // short_or: rows (false,false), (false,true), (true,false), (true,true).
                {
                    boolean value = new boolean[] {false, true, true, true}[row];
                    check(p.short_or_formals(a, b), value);
                    check(p.short_or_operands(a, b), value);
                    check(p.short_or_local(a, b), value);
                    check(p.short_or_match(a, b), value);
                    check(p.short_or_if(a, b), value);
                    check(p.short_or_literals(a, b), true);
                    check(p.short_or_nested(a, b), !value || (a && b));
                    check(p.short_or_guard(a, b), value ? 1 : 2);
                    check(p.short_or_while(a, b), value ? 2 : 0);
                    p.short_or_discard(a, b); checks++;
                    check(p.short_or_return(a, b), a ? value : b);
                    check(p.short_or_order(a, b), a ? 1 : 12);
                    check(p.short_or_discard_order(a, b), a ? 1 : 12);
                    if (!b) {
                        if (!a) throwsArithmetic(() -> p.short_or_throw(a, 0));
                        else check(p.short_or_throw(a, 0), true);
                        check(p.short_or_throw(a, 2), true);
                    }
                    if (!a && !b) throwsArithmetic(() -> p.short_or_after_return(a, b, 0));
                    else check(p.short_or_after_return(a, b, 0), a ? 3 : 7);
                    check(p.short_or_after_return(a, b, 2), a ? 3 : 7);
                }
            }
        }
        System.out.println("OK " + checks + " checks");
    }
}
