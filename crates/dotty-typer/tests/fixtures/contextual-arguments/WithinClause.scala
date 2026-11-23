package within
trait Family { type Out }
class Client:
  def read(using f: Family, v: f.Out): f.Out = v
  def curried(using f: Family)(using v: f.Out): f.Out = v
  def split(using p: Family, v: p.Out): p.Out = curried
  def explicit(using p: Family, v: p.Out): p.Out = read(using p, v)
