class C {
  def outer: Int = {
    def first = second
    def second: Int = first
    first
  }
}
