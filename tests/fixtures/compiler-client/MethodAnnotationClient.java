import demo.Base;
import demo.Relay;

class MethodAnnotationClient {
    public static void main(String[] args) {
        Relay relay = new Relay();
        Base first = new Base();
        Base second = new Base();
        System.out.println(relay.pass(first) == first && relay.pass(second) == second);
    }
}
