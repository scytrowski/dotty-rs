class Runner:
  def floatZero(): Float = 0.0f
  def floatOne(): Float = 1.0f
  def floatTwo(): Float = 2.0f
  def doubleZero(): Double = 0.0
  def doubleOne(): Double = 1.0
  def floatConstant(): Float = 1.5f
  def doubleConstant(): Double = 3.5

  def identityFloat(value: Float): Float = value
  def identityDouble(value: Double): Double = value

  def roundTripFloat(value: Float): Float =
    val saved: Float = value
    saved

  def roundTripDouble(value: Double): Double =
    val saved: Double = value
    saved

  def reassignedFloat(value: Float, next: Float): Float =
    var saved: Float = value
    saved = next
    saved

  def reassignedDouble(value: Double, next: Double): Double =
    var saved: Double = value
    saved = next
    saved

  def mixed(value: Float, amount: Double, tail: Long): Double =
    val saved: Double = amount
    val savedTail: Long = tail
    saved

  def explicitFloat(value: Float): Float = return value
  def explicitDouble(value: Double): Double = return value

  def discardedFloat(value: Float): Float =
    value
    1.5f
