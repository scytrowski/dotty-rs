package within
trait Family { type Out }
class Product
class TC[A]
class Built[A]
object Makers:
  given product(using f: Family, v: f.Out): Product = new Product
  given lifted[A](using f: Family, v: f.Out, tag: TC[A]): Built[A] = new Built[A]
object Api:
  export Makers.product
object Generic:
  export Makers.lifted
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
  def viaApi(using p: Family, v: p.Out): Product = { import Api.given; need }
  def named(using p: Family, v: p.Out): Product = Api.product(using p, v)
  def partial(using p: Family, v: p.Out): p.Out = read(using p)
  def oldDirect(using p: Family, v: p.Out): Product = { import Legacy.*; need }
  def generic(using p: Family, v: p.Out, tag: TC[Int]): Built[Int] = Generic.lifted[Int](using p, v, tag)
