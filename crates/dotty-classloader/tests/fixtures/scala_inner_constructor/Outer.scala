package constructors

class Outer[A]:
  class Inner(value: A):
    def outer: Outer[A] = Outer.this

class Pair[A](first: Outer[A], value: A)
