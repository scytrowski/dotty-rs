package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int): Pair =
    val second: Counter = new Counter(secondSeed)
    val first: Counter = new Counter(firstSeed)
    val alias: Counter = first
    alias.set(secondSeed)
    second.set(firstSeed)
    new Pair(first, second)
