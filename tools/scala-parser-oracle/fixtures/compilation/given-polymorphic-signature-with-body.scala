object NamedTupleOrdering:
  given namedTupleOrdering: [N <: Tuple, V <: Tuple] => (ord: Ordering[V]) => Ordering[NamedTuple[N, V]]:
    def compare(x: NamedTuple[N, V], y: NamedTuple[N, V]): Int =
      ord.compare(x.toTuple, y.toTuple)

  def next = 2
