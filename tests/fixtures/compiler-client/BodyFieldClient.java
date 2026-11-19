import demo.Counter;
import demo.UseCounter;

class BodyFieldClient {
    public static void main(String[] args) {
        UseCounter use = new UseCounter();
        Counter first = use.create(4);
        Counter second = use.create(2);
        System.out.println(use.read(first));
        use.update(first, 7);
        System.out.println(use.initial(first));
        System.out.println(use.read(first));
        System.out.println(use.read(second));
    }
}
