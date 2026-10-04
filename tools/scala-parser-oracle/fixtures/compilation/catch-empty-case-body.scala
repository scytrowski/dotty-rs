object EmptyCatch:
  def f =
    {
      def append =
        try
          risky()
        catch
          case _: RuntimeException =>

      val after = 1
    }
