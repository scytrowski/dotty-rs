package clients
import defaults.PairReader

class AppliedPairAmbiguous:
  def reader(): PairReader = new PairReader
  def ambiguous(using a: Int, b: Int): Int = reader().pick(1, 2)
  def missingSeed(): Int = reader().choose()
  def ambiguousSeed(using a: Int, b: Int): Int = reader().choose()
