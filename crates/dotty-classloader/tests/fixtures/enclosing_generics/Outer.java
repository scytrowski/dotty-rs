package enclosing_generics;

public class Outer<K> {
    public class Inner {
        public K value;

        public K get() {
            return value;
        }
    }
}
