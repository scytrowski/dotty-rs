if condition then Nil else
  xs.flatMap(x =>
    f(x)).map(y =>
    g(y))
