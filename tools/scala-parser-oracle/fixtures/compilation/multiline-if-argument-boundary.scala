object MultilineIfArgumentBoundary:
  def check(classSym: Int): Unit =
    assert(
      if (classSym == 0)
        true
      else if (classSym == 1)
        false
      else
        true,
      s"bad $classSym"
    )
    val after = classSym
