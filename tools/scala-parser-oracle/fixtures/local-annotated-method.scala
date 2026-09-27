{
  def previous = { 1 }
  @tailrec def local(n: Int): Int = if n == 0 then previous else local(n - 1)
  local(2)
}
