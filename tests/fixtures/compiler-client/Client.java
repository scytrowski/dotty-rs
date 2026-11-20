import demo.A;
import demo.B;

class Client {
    public static void main(String[] args) {
        System.out.println(new B().read(new A()));
    }
}
