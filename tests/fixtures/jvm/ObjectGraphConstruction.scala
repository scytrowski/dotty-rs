class Counter(initial: Int):
  private var current: Int = initial
  def get(): Int = current
  def increment(): Int =
    current = current + 1
    current

class Holder(counter: Counter):
  private val inner: Counter = counter
  def get(): Int = inner.get()
  def value(): Counter = inner

class OwnedHolder(seed: Int):
  private val inner: Counter = new Counter(seed)
  def get(): Int = inner.get()
  def value(): Counter = inner

class Pair(first: Counter, second: Counter):
  private val savedFirst: Counter = first
  private val savedSecond: Counter = second
  def firstCounter(): Counter = savedFirst
  def secondCounter(): Counter = savedSecond
  def firstValue(): Int = savedFirst.get()
  def secondValue(): Int = savedSecond.get()

class Runner:
  private var receiverCalls: Int = 0
  def makeCounter(seed: Int): Counter =
    receiverCalls = receiverCalls + 1
    new Counter(seed)
  def makeHolder(seed: Int): Holder =
    receiverCalls = receiverCalls + 1
    new Holder(new Counter(seed))
  def create(seed: Int): Holder =
    receiverCalls = receiverCalls + 1
    new Holder(new Counter(seed))
  def receiverCallCount(): Int = receiverCalls
  def fromFactory(seed: Int): Holder = new Holder(makeCounter(seed))
  def existing(counter: Counter): Holder = new Holder(counter)
  def fromImmutable(counter: Counter): Holder =
    val current: Counter = counter
    new Holder(current)
  def fromMutable(counter: Counter): Holder =
    var current: Counter = counter
    new Holder(current)
  def fromLocal(seed: Int): Holder =
    var current: Counter = new Counter(seed)
    new Holder(current)
  def fromNull(): Holder = new Holder(null)
  def createPair(first: Int, second: Int): Pair =
    new Pair(new Counter(first), new Counter(second))
  def read(seed: Int): Int = create(seed).value().get()
  def chainedFactory(seed: Int): Int = makeHolder(seed).value().get()
  def readOwned(seed: Int): Int = new OwnedHolder(seed).get()
