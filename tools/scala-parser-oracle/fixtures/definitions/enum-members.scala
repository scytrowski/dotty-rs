{
  enum Box {
    val value = 1
    def current = value
    type Item = Int
    given Ordering = ordering
  }
}
