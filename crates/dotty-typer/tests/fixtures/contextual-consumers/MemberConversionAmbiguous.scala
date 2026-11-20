package memberconversions

class Also { def show: Int = 3 }
class Ambiguous {
  def ambiguous(x: From)(using first: Conversion[From, To], second: Conversion[From, Also]): Int = x.show
}
