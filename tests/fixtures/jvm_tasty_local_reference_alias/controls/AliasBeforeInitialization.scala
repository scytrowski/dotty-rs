package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int): Pair =
    val alias: Counter = first
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    alias.set(secondSeed)
    second.set(firstSeed)
    new Pair(first, second)
