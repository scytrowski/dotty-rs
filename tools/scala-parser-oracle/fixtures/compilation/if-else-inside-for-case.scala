def f(pairs: List[(Int, Int)]) =
  for case (arg, formal) <- pairs do
    if test(arg) then
      set(arg)
      else
        transform(formal)
