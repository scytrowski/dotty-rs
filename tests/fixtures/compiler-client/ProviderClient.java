import demo.A;
import demo.B;
import demo.Use;

class ProviderClient {
    public static void main(String[] args) {
        Use use = new Use();
        System.out.println(use.fromA(new A()));
        System.out.println(use.fromB(new B()));
        System.out.println(use.diff(9, 4));
    }
}
