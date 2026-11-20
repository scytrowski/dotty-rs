import demo.Counter;

class CounterClient {
    public static void main(String[] args) {
        Counter first = new Counter();
        Counter second = new Counter();
        System.out.println(first.value());
        first.add(3);
        System.out.println(first.value());
        first.add(-1);
        System.out.println(first.value());
        System.out.println(second.value());
    }
}
