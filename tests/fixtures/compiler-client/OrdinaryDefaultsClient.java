public class OrdinaryDefaultsClient {
    static class Traced extends demo.Ops {
        final StringBuilder trace = new StringBuilder();
        @Override public demo.Ops receiver() { trace.append("O"); return this; }
        @Override public int supplied() { trace.append("r"); return 4; }
        @Override public int left() { trace.append("L"); return 9; }
        @Override public int right() { trace.append("R"); return 4; }

        void check(int value, String expected) {
            if (value != 5 || !trace.toString().equals(expected))
                throw new AssertionError(value + " " + trace + " expected " + expected);
            trace.setLength(0);
        }
    }

    public static void main(String[] args) {
        Traced o = new Traced();
        o.check(o.partial(), "OrL");
        o.check(o.all(), "OLR");
        o.check(o.explicit(), "Or");
        o.check(new demo.Use().cross(o), "L");
        System.out.println("OK");
    }
}
