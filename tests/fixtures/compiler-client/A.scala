package demo

class A:
  def get() = { val value = 5; value + { val value = 2; value } }
