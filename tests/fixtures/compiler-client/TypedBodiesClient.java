import demo.A;
import demo.B;

class TypedBodiesClient {
    public static void main(String[] args) {
        System.out.println(new A().value());
        System.out.println(new B().value());
    }
}
