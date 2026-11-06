package example;

public final class RuntimeProbe {
    public static void main(String[] args) {
        Pair pair = new PairFactory().create(7, 3);
        System.out.println(pair.leftValue() + "/" + pair.rightValue());
    }
}
