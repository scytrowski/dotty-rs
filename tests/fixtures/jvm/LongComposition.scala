class LongBox(initial: Long):
  private var current: Long = initial
  def get(): Long = current
  def set(value: Long): Unit =
    current = value
  def select(prefix: Int, left: Long, enabled: Boolean, right: Long): Long =
    if enabled then left else right

class LongDerived(seed: Long) extends LongBox(seed)
class LongDerivedLiteral extends LongBox(9000000000L)

class Runner:
  def choose(flag: Boolean, left: Long, right: Long): Long =
    if flag then left else right

  def loop(n: Int, first: Long, second: Long): Long =
    var current: Long = first
    var i = 0
    while i < n do
      current = second
      i = i + 1
    current

  def forward(box: LongBox): Long = box.get()

  def mixed(box: LongBox, enabled: Boolean, left: Long, right: Long): Long =
    box.select(7, left, enabled, right)

  def roundTrip(seed: Long): Long =
    val box: LongBox = new LongBox(seed)
    box.get()

  def literalRoundTrip(): Long =
    new LongBox(9000000000L).get()
