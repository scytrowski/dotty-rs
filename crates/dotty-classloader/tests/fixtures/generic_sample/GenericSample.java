import java.util.List;

public class GenericSample<T extends Comparable<T>> {
    public List<T> items;

    public T first() {
        return items.get(0);
    }
}
