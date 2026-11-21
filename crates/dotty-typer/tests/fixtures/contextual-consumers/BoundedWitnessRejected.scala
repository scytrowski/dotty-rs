package boundedwitness
class Rejected extends Client {
  def outside(x: Box[Int], b: Other)(using tc: TC[Int], f: Format[Other], t: Trace[Other]): Other = x.keep(b)
  def outsideOnly(x: Box[Int], b: Other)(using tc: TC[Int], f: Format[Other], t: Trace[Other]): Other = x.only(b)
}
