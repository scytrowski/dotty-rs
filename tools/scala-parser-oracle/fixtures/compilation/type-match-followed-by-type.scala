type Zip[X <: Tuple, Y <: Tuple] =
  X match
    case Y =>
      (X, Y)

type Next[T] = T
