package within
trait Family { type Out }
class Product
object Makers:
  given product(using f: Family, v: f.Out): Product = new Product
object Legacy:
  implicit def old(implicit f: Family, v: f.Out): Product = new Product
class Client:
  def read(using f: Family, v: f.Out): f.Out = v
  def curried(using f: Family)(using v: f.Out): f.Out = v
  def need(using x: Product): Product = x
  def split(using p: Family, v: p.Out): p.Out = curried
  def explicit(using p: Family, v: p.Out): p.Out = read(using p, v)
  def inferred(using p: Family, v: p.Out): p.Out = read
  def original(using p: Family, v: p.Out): Product = { import Makers.product; need }
  def oldDirect(using p: Family, v: p.Out): Product = { import Legacy.*; need }
