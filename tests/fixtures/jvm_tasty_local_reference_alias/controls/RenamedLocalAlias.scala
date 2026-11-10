package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int): Pair =
    val primary: Counter = new Counter(firstSeed)
    val secondary: Counter = new Counter(secondSeed)
    val original: Counter = primary
    original.set(secondSeed)
    secondary.set(firstSeed)
    new Pair(primary, secondary)
