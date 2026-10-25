class InlineFlags {
  def hasFinalFlag(flags: Int): Boolean = (flags & 1) != 0
  def mask(flags: Int, bits: Int): Int = flags & bits
}
