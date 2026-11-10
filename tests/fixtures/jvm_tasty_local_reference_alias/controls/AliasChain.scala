package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    val alias: Counter = first
    val chained: Counter = alias
    chained.set(secondSeed)
    second.set(firstSeed)
    new Pair(first, second)
