class NestedEvidence:
  def read(using first: Int, second: Boolean): Int = first
  def readOne(using value: Int): Int = value
  def nested: Int ?=> Boolean ?=> Int = read
  def same: Int ?=> Int ?=> Int = readOne
  def fromPlain(x: Int): Boolean ?=> Long ?=> Int = x
