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
