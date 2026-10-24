class NumericBox(initialF: Float, initialD: Double):
  private var f: Float = initialF
  private var d: Double = initialD
  def getF(): Float = f
  def getD(): Double = d
  def setF(value: Float): Unit = f = value
  def setD(value: Double): Unit = d = value

class NumericChild(initialF: Float, initialD: Double) extends NumericBox(initialF, initialD)

class Runner:
  def identityF(value: Float): Float = value
  def identityD(value: Double): Double = value

  def pickF(flag: Boolean, left: Float, right: Float): Float =
    if flag then left else right

  def pickD(flag: Boolean, left: Double, right: Double): Double =
    if flag then left else right

  def loop(n: Int, first: Double, second: Double): Double =
    var current: Double = first
    var i = 0
    while i < n do
      current = second
      i = i + 1
    current

  def forward(box: NumericBox): Double = box.getD()

  def construct(f: Float, d: Double): Double =
    val box: NumericBox = new NumericBox(f, d)
    box.getD()

  def constructNested(f: Float, d: Double): Double =
    val box: NumericBox = new NumericBox(identityF(f), identityD(d))
    box.getD()

  def mixed(i: Int, f: Float, d: Double, l: Long): Double = d
