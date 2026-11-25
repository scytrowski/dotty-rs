public class NamedPrimaryClient extends demo.Probe {
    private final StringBuilder trace = new StringBuilder();

    @Override public int left() { trace.append("L"); return 9; }
    @Override public int right() { trace.append("R"); return 4; }

    public static void main(String[] args) {
        NamedPrimaryClient probe = new NamedPrimaryClient();
        int code = probe.make(probe).code();
        System.out.println(code + " " + probe.trace);
    }
}
