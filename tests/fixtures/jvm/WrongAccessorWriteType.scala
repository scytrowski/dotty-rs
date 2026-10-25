class Counter(val id: Int, var count: Long):
  def wrongWrite(next: Boolean): Unit = this.count = next
