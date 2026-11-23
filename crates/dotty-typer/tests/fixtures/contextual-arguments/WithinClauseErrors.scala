package within
class Bad:
  def read(using f: Family, v: f.Out): f.Out = v
  def need(using x: Product): Product = x
  def wrong(p: Family, q: Family)(v: q.Out): p.Out = read(using p, v)
  def noFirst: Product = { import Api.given; need }
  def noSecond(using p: Family): Product = { import Api.given; need }
  def ambiguous(using p: Family, a: p.Out, b: p.Out): Product = { import Again.given; need }
  def wrongSearch(q: Family)(using p: Family, other: q.Out): Product = { import Makers.product; need }
  def partialMissing(p: Family): p.Out = read(using p)
  def partialAmbiguous(p: Family)(using a: p.Out, b: p.Out): p.Out = read(using p)
