public class InitializerCallClient {
    public static void main(String[] args) {
        System.out.println(new Consumer(new Provider(new Seeds())).answer());
    }
}
