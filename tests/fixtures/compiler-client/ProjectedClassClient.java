import demo.A;
import demo.B;
import demo.Consumer;

class ProjectedClassClient {
    public static void main(String[] args) {
        Consumer consumer = new Consumer();
        System.out.println(consumer.first(new A()));
        System.out.println(consumer.second(new B()));
    }
}
