object CheckUnused:
  extension [A](array: Array[A])
    def indexSatisfying(from: Int, until: Int)(predicate: A => Boolean) =
      var index = from
      while index < until && !predicate(array(index)) do
        index += 1
      index
end CheckUnused
