type Concat[X <: Tuple, +Y <: Tuple] <: Tuple = X match {
  case EmptyTuple => Y
  case x1 *: xs1 => x1 *: Concat[xs1, Y]
}
