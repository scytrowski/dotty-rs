package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val selected: Counter =
      if chooseFirst then first else later
    val later: Counter = new Counter(secondSeed)
    selected.set(nextValue)
    new Pair(first, later)
