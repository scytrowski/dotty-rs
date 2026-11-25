public class MixedConstructorClient extends demo.Probe {
    private final StringBuilder trace = new StringBuilder();

    @Override public int count() { trace.append("C"); return 4; }
    @Override public int seed() { trace.append("S"); return 9; }
    @Override public int id() { trace.append("I"); return 2; }

    public static void main(String[] args) throws ReflectiveOperationException {
        var probe = new MixedConstructorClient();
        var first = probe.make(probe);
        var second = probe.make(probe);
        int before = probe.read(first);
        probe.update(first, 7);
        System.out.println(probe.trace + " " + first.initial() + " " + first.id()
                + " " + before + " " + probe.read(first) + " " + probe.read(second));

        var type = demo.Counter.class;
        type.getConstructor(int.class, int.class, int.class);
        if (type.getMethod("id").getReturnType() != int.class
                || type.getMethod("count").getReturnType() != int.class
                || type.getMethod("count_$eq", int.class).getReturnType() != void.class)
            throw new AssertionError("unexpected Counter accessors");
        for (var method : type.getMethods())
            if (method.getName().equals("id_$eq"))
                throw new AssertionError("setter for immutable id");
    }
}
