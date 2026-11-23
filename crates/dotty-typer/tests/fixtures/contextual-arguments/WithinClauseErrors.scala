package within
class Bad:
  def read(using f: Family, v: f.Out): f.Out = v
  def need(using x: Product): Product = x
  def wrong(p: Family, q: Family)(v: q.Out): p.Out = read(using p, v)
  def wrongSearch(q: Family)(using p: Family, other: q.Out): Product = { import Makers.product; need }
