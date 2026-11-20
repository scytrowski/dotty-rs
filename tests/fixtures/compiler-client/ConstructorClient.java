import demo.Counter;
import demo.Pair;

class ConstructorClient {
    public static void main(String[] args) {
        Counter first = new Counter(40);
        Counter second = new Counter(7);
        System.out.println(first.value());
        System.out.println(second.value());
        first.add(2);
        System.out.println(first.value());
        System.out.println(second.value());
        System.out.println(new Pair(9, 4).difference());
    }
}
