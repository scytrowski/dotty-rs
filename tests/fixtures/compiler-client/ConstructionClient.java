import demo.Factory;

class ConstructionClient {
    public static void main(String[] args) {
        Factory factory = new Factory();
        System.out.println(factory.build(40));
        System.out.println(factory.build(7));
    }
}
