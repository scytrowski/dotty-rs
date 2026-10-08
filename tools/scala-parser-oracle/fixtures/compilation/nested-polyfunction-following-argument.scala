object O:
  def run =
    val first = invoke(
      handle = [A] =>
        (value: A) =>
          locally {
            val result = value
            result
          },
      done = value => value
    )
    val after = true
  def outside = true
