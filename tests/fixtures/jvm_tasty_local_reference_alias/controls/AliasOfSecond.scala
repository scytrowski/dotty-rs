package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    val alias: Counter = second
    alias.set(firstSeed)
    first.set(secondSeed)
    new Pair(first, second)
