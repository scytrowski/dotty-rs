for { x <- xs; y = f(x); if pred(y); z <- zs(y) } yield z
