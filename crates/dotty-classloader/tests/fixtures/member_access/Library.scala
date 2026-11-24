package audits
sealed class Ledger:
  private[audits] val amount: Int = 1
  private[audits] def read(x: Int): Int = amount + x
  protected[audits] def check(x: Int): Int = x
  def nullary: Int = 2
  def open(x: Int): Int = x
  val shown: Int = 1
class PublicArg(val exposed: Int)
