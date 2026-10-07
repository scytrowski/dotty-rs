class C {
  def outer: Int = {
    extension (receiver: C) { def choose(argument: Int): Int = 1 }
    this.choose(2)
  }
}
