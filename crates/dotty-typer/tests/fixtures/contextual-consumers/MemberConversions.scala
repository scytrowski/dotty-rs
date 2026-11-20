package memberconversions

class From
class Own { def show: Int = 1 }
class To { def show: Int = 2 }
class Client {
  def direct(x: Own)(using ignored: Conversion[Own, To]): Int = x.show
  def converted(x: From)(using via: Conversion[From, To]): Int = x.show
}
