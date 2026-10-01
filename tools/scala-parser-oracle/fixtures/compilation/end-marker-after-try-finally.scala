object EndMarkerAfterTryFinally:
  class MutConsumedSet:
    def segment(op: => Unit): Int =
      val start = size
      try
        op
        if size == start then Empty
        else Const()
      finally
        size = start
  end MutConsumedSet
end EndMarkerAfterTryFinally
