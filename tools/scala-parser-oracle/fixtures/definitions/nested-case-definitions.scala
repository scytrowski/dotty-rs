{
  class Outer:
    case class Inner(value: A):
      def get = value
    case object Empty
    val done = true
}
