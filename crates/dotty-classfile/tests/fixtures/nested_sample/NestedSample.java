public class NestedSample {
    @Deprecated(since = "1.0", forRemoval = true)
    public static class Inner {
        public Inner() {
        }
    }

    public static <T extends Comparable<T>> T max(T a, T b) throws IllegalStateException {
        if (a == null || b == null) {
            throw new IllegalStateException("null argument");
        }
        return a.compareTo(b) >= 0 ? a : b;
    }

    public Runnable makeLocalRunnable(String label) {
        class LocalRunnable implements Runnable {
            public void run() {
                System.out.println(label);
            }
        }
        return new LocalRunnable();
    }
}
