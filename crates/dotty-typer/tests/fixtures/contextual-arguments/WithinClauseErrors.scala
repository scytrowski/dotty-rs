package within
class Bad:
  def read(using f: Family, v: f.Out): f.Out = v
  def wrong(p: Family, q: Family)(v: q.Out): p.Out = read(using p, v)
