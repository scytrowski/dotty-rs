package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseSecond: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    var selected: Counter = first
    if chooseSecond then
      selected = later
    else
      selected = first
    val later: Counter = new Counter(secondSeed)
    selected.set(nextValue)
    new Pair(first, later)
