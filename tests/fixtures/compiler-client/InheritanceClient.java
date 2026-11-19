import demo.Base;
import demo.Derived;

class InheritanceClient {
    public static void main(String[] args) {
        Base base = new Base();
        Base derived = new Derived();
        System.out.println(base.read());
        System.out.println(derived.initialized());
        System.out.println(derived.read());
    }
}
