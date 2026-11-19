class NestedEvidenceMissing:
  def read(using first: Int, second: Boolean, third: Long): Int = first
  def missing: Int ?=> Boolean ?=> Int = read
  def recovered(using third: Long): Int ?=> Boolean ?=> Int = read
