package clients
import defaults.PairReader

class AppliedPairClient:
  def reader(): PairReader = new PairReader
  def direct(): Int = reader().pick(1, 2)(using 9)
  def left(): Int = 13
  def right(): Int = 29
  def fallback(): Int = reader().pick(left(), right())
