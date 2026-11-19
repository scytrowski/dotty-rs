class CurriedDefault:
  def root(using c: Int): Int = c
  def narrow(using c: Int)(owner: Int = root): Int = owner
  def test(using c: Int): Int = narrow()
