public class NamedParentClient {
    public static void main(String[] args) {
        demo.Base base = new demo.Derived(9, 4);
        System.out.println(base.id() + " " + base.count());
    }
}
