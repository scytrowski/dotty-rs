class ParameterizedBase(seed: Int, enabled: Boolean):
  private var current: Int = seed
  private val active: Boolean = enabled

  def value(): Int = current
  def isActive(): Boolean = active

class ParameterizedDerived(start: Int, active: Boolean) extends ParameterizedBase(start, active):
  private val original: Int = start

  def originalValue(): Int = original

class LiteralBase(seed: Int, enabled: Boolean):
  private val stored: Int = seed
  private val active: Boolean = enabled

  def value(): Int = stored
  def isActive(): Boolean = active

class LiteralDerived extends LiteralBase(123456, true)

class Dep

class ReferenceBase(dep: Dep)

class ReferenceDerived(dep: Dep) extends ReferenceBase(dep)
