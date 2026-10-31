package inheritancetypernegative

class First:
  def add(x: Int): Int = x + 1
class Second:
  def add(x: Int): Int = x + 2
class Use:
  def first(value: First): Int = value.add(1)
  def second(value: Second): Int = value.add(1)
