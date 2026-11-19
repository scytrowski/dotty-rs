import demo.Base;
import demo.Derived;

class ForwardingClient {
    public static void main(String[] args) {
        Base first = new Derived(40);
        Base second = new Derived(7);
        System.out.println(first.read());
        System.out.println(second.read());
    }
}
