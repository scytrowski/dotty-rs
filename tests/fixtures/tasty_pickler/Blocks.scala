package example

class Blocks:
  def ordered(flag: Boolean, value: Int): Int =
    {
      if flag then 1 else 2
      value
    }
  def single: Int = { identity(7) }
  def calls: Int =
    {
      identity(1)
      identity(2)
      identity(3)
    }
  def identity(value: Int): Int = value
