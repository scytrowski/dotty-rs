class Counter(initial: Int):
  private var current: Int = initial
  def get(): Int = current

class Holder(counter: Counter):
  private val inner: Counter = counter
  def get(): Int = inner.get()
  def value(): Counter = inner

class OwnedHolder(seed: Int):
  private val inner: Counter = new Counter(seed)
  def get(): Int = inner.get()

class Runner:
  private var receiverCalls: Int = 0
  def makeCounter(seed: Int): Counter = new Counter(seed)
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
  def fromNull(): Holder = new Holder(null)
  def read(seed: Int): Int = create(seed).value().get()
  def chainedFactory(seed: Int): Int = makeHolder(seed).value().get()
  def readOwned(seed: Int): Int = new OwnedHolder(seed).get()
