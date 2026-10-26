public final class InheritedAccessorsHarness {
  public static void main(String[] args) {
    Child child = new Child();
    if (child.id() != 42L || child.current() != 1) throw new AssertionError();
    if (child.read() != 1 || child.readThis() != 1) throw new AssertionError();
    if (child.readChild(child) != 1 || child.readParent(child) != 1) throw new AssertionError();
    child.write(8);
    if (child.current() != 8 || child.read() != 8) throw new AssertionError();
    child.writeThis(9);
    if (child.readParent(child) != 9) throw new AssertionError();
    Leaf leaf = new Leaf();
    leaf.current_$eq(17);
    if (leaf.current() != 17 || leaf.id() != 42L || leaf.read() != 17) throw new AssertionError();
    Reader reader = new Reader();
    if (reader.readChild(leaf) != 17 || reader.readLeaf(leaf) != 17 || reader.readParent(leaf) != 17) {
      throw new AssertionError();
    }
    AccessorChild accessorChild = new AccessorChild();
    if (accessorChild.token() != 7 || accessorChild.count() != 9L) throw new AssertionError();
    if (accessorChild.readToken() != 7 || accessorChild.readCount() != 9L) throw new AssertionError();
    accessorChild.count_$eq(Long.MAX_VALUE);
    if (accessorChild.count() != Long.MAX_VALUE || accessorChild.readCount() != Long.MAX_VALUE) throw new AssertionError();
    reader.writeCount(accessorChild, Long.MIN_VALUE);
    if (reader.readToken(accessorChild) != 7 || reader.readCount(accessorChild) != Long.MIN_VALUE) {
      throw new AssertionError();
    }
  }
}
