package example;

// Regenerate Bounded.class beside this source with javac --release 21 -g:none Bounded.java.
public class Bounded {
    public <T extends Number> T ident(T value) {
        return value;
    }
}
