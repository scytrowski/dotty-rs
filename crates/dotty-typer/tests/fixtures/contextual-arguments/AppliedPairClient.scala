package clients
import defaults.PairReader

class AppliedPairClient:
  def reader(): PairReader = new PairReader
  def direct(): Int = reader().pick(1, 2)(using 9)
