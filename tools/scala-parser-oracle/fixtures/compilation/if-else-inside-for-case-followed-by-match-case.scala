def f(args: List[Int]) =
  args match
    case xs =>
      for case x <- xs do
        if test(x) then
          one(x)
          else
            two(x)
    case _ =>
      fallback
