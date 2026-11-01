package inheritedmembers

class OverloadedParent:
  def add(x: Int): Int = x
  def add(x: String): String = x

class OverloadedChild extends OverloadedParent:
  def run(x: Int): Int = add(x)

class GenericParent:
  def add[A](x: A): Int = 0

class GenericChild extends GenericParent:
  def run(x: Int): Int = add(x)
