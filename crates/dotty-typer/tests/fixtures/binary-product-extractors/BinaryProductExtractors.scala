case class PairResult(_1: Int, _2: Boolean)

class MaybePair:
  def isEmpty: Boolean = false
  def get: PairResult = PairResult(1, true)

object DirectPair:
  def unapply(value: Any): PairResult = PairResult(1, true)

object GetPair:
  def unapply(value: Any): MaybePair = new MaybePair

object BinaryProductExtractors:
  def direct(value: Any): Int = value match
    case DirectPair(left, right) => left

  def throughGet(value: Any): Boolean = value match
    case GetPair(_, right) => right
