package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseSecond: Boolean, nextValue: Int): Pair =
    val primary: Counter = new Counter(firstSeed)
    val secondary: Counter = new Counter(secondSeed)
    var chosen: Counter = primary
    if chooseSecond then
      chosen = secondary
    else
      chosen = primary
    chosen.set(nextValue)
    new Pair(primary, secondary)
