class NumericBox(initialF: Float, initialD: Double):
  private var f: Float = initialF
  private var d: Double = initialD
  def getF(): Float = f
  def getD(): Double = d
  def setF(value: Float): Unit = f = value
  def setD(value: Double): Unit = d = value

class NumericConstants:
  private val f: Float = 1.5f
  private val d: Double = 3.5
  def getF(): Float = f
  def getD(): Double = d

class Runner:
  def floatValue(): Float = 1.5f
  def doubleValue(): Double = 3.5

  def roundTripF(value: Float): Float =
    val saved: Float = value
    saved

  def roundTripD(value: Double): Double =
    val saved: Double = value
    saved

  def replaceD(value: Double, next: Double): Double =
    var saved: Double = value
    saved = next
    saved

  def chooseF(flag: Boolean, left: Float, right: Float): Float =
    if flag then left else right

  def chooseD(flag: Boolean, left: Double, right: Double): Double =
    if flag then left else right

  def loop(n: Int, first: Double, second: Double): Double =
    var current: Double = first
    var i = 0
    while i < n do
      current = second
      i = i + 1
    current

  def forwardD(box: NumericBox): Double = box.getD()

  def construct(f: Float, d: Double): Double =
    val box: NumericBox = new NumericBox(f, d)
    box.getD()

  def mixed(i: Int, f: Float, d: Double, l: Long): Double = d
