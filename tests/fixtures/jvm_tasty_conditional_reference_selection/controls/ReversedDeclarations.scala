package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
    val second: Counter = new Counter(secondSeed)
    val first: Counter = new Counter(firstSeed)
    val selected: Counter =
      if chooseFirst then first else second
    selected.set(nextValue)
    new Pair(first, second)
