object O:
  def run =
    val first = outer: x =>
      inner: y =>
        val combined = x + y
        combined
      after(x)
    val next = true
  def outside = true
