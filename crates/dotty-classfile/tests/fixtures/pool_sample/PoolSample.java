public class PoolSample implements Runnable {
    public static final int ANSWER = 42;
    public static final long BIG_ANSWER = 42_000_000_000L;
    public static final float HALF = 0.5f;
    public static final double PI = 3.14159;
    public static final String GREETING = "hello";

    @Override
    public void run() {
        int value = computeAnswer();         // not a compile-time constant
        String message = GREETING + value;   // runtime string concat: MethodHandle/MethodType/InvokeDynamic
        System.out.println(message);         // Fieldref + Methodref
        Runnable self = this;
        self.run();                          // InterfaceMethodref
    }

    private static int computeAnswer() {
        return ANSWER;
    }
}
