class Counter(val id: Int, var count: Long):
  def immutableWrite(next: Int): Unit = this.id = next
