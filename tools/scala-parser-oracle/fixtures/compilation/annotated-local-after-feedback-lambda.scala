object AnnotatedLocalAfterFeedbackLambda:
  def reduce[A, B](first: A => B, second: A => B): B = first(null.asInstanceOf[A])

  def apply(bound: Int): Int =
    reduce(
      value =>
        val n = value
        + 1
        @scala.annotation.tailrec def loop(index: Int): Int =
          if index >= n then n else loop(index + 1)
        loop(0)
      ,
      value => value
    )
