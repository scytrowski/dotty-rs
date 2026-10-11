package enclosing_generics;

public class Outer<K> {
    public Inner inner;

    public class Inner {
        public K value;

        public K get() {
            return value;
        }
    }

    public class Shadow<K> {
        public K value;
    }

    public class MethodChild<U> {
        public <K> K echo(K value) {
            return value;
        }
    }

    public class Middle<V> {
        public class Deep<W> {
            public class Leaf<X> {
                public K outer;
                public V middle;
                public W near;
                public X own;
            }
        }
    }

    public static class Static<U> {
        public U value;
    }
}
