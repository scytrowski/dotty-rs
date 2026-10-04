class SingletonOuter:
  val value: Int = 1
  val same: value.type = value
  val self: this.type = this

  class Inner:
    val nested: Int = 2
    val outer: SingletonOuter.this.type = SingletonOuter.this

  val inner: Inner = new Inner
  val selected: inner.nested.type = inner.nested
