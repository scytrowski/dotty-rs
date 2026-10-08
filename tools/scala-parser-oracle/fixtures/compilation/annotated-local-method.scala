object Outer:
  def outer(xs: List[Int]): Int =
    val selected = xs match
      case Nil => 0
      case _   => 1
    // Keep an annotation after a match-valued local definition.
    @scala.annotation.tailrec
    def loop(n: Int): Int =
      if n == 0 then selected else loop(n - 1)
    loop(3)
