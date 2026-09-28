object Outer:
  def f =
    val sf =
      new Foo:
        def x = 1
    sf.foo
  def next = 2
