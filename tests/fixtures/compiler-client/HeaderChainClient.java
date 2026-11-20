import demo.Base;
import demo.Leaf;

class HeaderChainClient {
    public static void main(String[] args) {
        Base first = new Leaf(40);
        Base second = new Leaf(7);
        System.out.println(first.read());
        System.out.println(second.read());
    }
}
