package example.visibility;

// From this directory: javac --release 21 -g:none -d . NestedAccess.java
public class NestedAccess {
    private static class Hidden {
        int field;
        Hidden() {}
        void method() {}
    }

    protected static class Sub {}
}
