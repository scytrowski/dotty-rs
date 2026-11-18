package clients
import defaults.PairReader

class BinaryPairClient:
  def reader: PairReader = new PairReader
  def left(): Int = 13
  def right(): Int = 29
  def run(): Int = reader.pick(left(), right())
