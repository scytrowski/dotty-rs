package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    val selected: Counter =
      if chooseFirst then first else nextValue
    selected.set(nextValue)
    new Pair(first, second)
