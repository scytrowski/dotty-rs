class Counter(start: Int):
  private var count: Int = start
  def get(): Int = count

class Holder(initial: Counter):
  private val ref: Counter = initial
  private[this] val own: Counter = initial
  def get(): Counter = ref
  def read(): Int = ref.get()

class MutableHolder(initial: Counter):
  private var current: Counter = initial
  def replace(next: Counter): Unit = current = next
  def get(): Counter = current
  def read(): Int = current.get()

class Client:
  def construct(c: Counter): Holder = new Holder(c)
