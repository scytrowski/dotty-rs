import demo.Counter;
import demo.UseCounter;

class AccessorClient {
    public static void main(String[] args) {
        UseCounter use = new UseCounter();
        Counter first = use.create(9, 4);
        Counter second = use.create(1, 2);
        System.out.println(first.id());
        System.out.println(use.read(first));
        use.update(first, 7);
        System.out.println(use.read(first));
        System.out.println(use.read(second));
    }
}
