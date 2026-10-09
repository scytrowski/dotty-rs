class LiteralSingletons:
  val boolTrue: true = true
  val boolFalse: false = false
  val intOne: 1 = 1
  val stringFoo: "foo" = "foo"
  val boolUnderlying: Boolean = true
  val falseUnderlying: Boolean = false
  val intUnderlying: Int = 1
  val stringUnderlying: String = "foo"

  def sameBoolean(value: true): true = value
  def sameBooleanFalse(value: false): false = value
  def sameInt(value: 1): 1 = value
  def sameString(value: "foo"): "foo" = value
