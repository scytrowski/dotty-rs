{
  def flags(inline op: Boolean, normal: Int): Int = normal
  def named(inline: Int): Int = inline
  def contextual(using inline ordering: Ordering[Int]): Int = 1
  given (using inline evidence: Evidence) => Service = makeService
  class C(inline value: Boolean)
  extension (inline receiver: Boolean)
    def check(using inline context: Context): Boolean = receiver
}
