package example

class PairFactoryAlias:
  def create(firstSeed: Int, secondSeed: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    new Pair(first, first)
