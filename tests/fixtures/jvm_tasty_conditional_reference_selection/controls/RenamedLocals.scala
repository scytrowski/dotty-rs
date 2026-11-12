package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseFirst: Boolean, nextValue: Int): Pair =
    val primary: Counter = new Counter(firstSeed)
    val secondary: Counter = new Counter(secondSeed)
    val chosen: Counter =
      if chooseFirst then primary else secondary
    chosen.set(nextValue)
    new Pair(primary, secondary)
