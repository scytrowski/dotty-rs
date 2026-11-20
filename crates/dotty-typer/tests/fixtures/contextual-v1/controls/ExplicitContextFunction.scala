class ExplicitContextFunction:
  inline def foo: Int = bar(using 4)
  private def bar: Int ?=> Int = 3
