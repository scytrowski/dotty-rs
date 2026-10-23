class LongBox(initial: Long):
  private var current: Long = initial

  def get(): Long = current
  def set(value: Long): Unit =
    current = value

class LongConstants:
  private val large: Long = 9000000000L
  def value(): Long = large

class Runner:
  def zero(): Long = 0L
  def one(): Long = 1L
  def constant(): Long = 9000000000L

  def roundTrip(value: Long): Long =
    val saved: Long = value
    saved

  def reassigned(value: Long, next: Long): Long =
    var saved: Long = value
    saved = next
    saved

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

  def construct(seed: Long): Long =
    val box: LongBox = new LongBox(seed)
    box.get()
