class C {
  def outer(value: Int): Int = {
    extension (receiver: Int) {
      def choose(argument: Int): Int = receiver
    }
    value.choose(1)
  }
}
