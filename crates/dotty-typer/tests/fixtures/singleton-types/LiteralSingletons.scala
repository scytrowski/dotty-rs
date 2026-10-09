class LiteralSingletons:
  val boolTrue: true = true
  val boolFalse: false = false
  val intOne: 1 = 1
  val stringFoo: "foo" = "foo"
  val boolUnderlying: Boolean = true
  val intUnderlying: Int = 1

  def sameBoolean(value: true): true = value
  def sameInt(value: 1): 1 = value
