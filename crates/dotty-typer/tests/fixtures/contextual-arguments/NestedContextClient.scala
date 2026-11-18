package clients

class NestedContextClient:
  def read(using n: Int, flag: Boolean): Int = if flag then n else 0
  def build: Int ?=> (Boolean ?=> Int) = read
  def explicit: Int = build(using 7)(using true)
  def inferred: Int =
    given number: Int = 7
    given flag: Boolean = true
    build
  def mixed(using flag: Boolean): Int = build(using 7)
  def partial: Boolean ?=> Int = build(using 7)
