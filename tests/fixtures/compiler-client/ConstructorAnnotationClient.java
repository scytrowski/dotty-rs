import demo.Base;
import demo.Holder;
import demo.Peer;

class ConstructorAnnotationClient {
    public static void main(String[] args) {
        Base first = new Base();
        Peer initial = new Peer();
        Holder holder = new Holder(new Base(), first, initial);
        boolean initialized = holder.first() == first && holder.next() == initial;
        Peer replacement = new Peer();
        holder.next_$eq(replacement);
        System.out.println(initialized && holder.next() == replacement && holder.first() == first);
    }
}
