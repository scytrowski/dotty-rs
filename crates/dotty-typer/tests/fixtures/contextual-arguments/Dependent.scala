package dependentclauses

class Family:
  class Item
  given member: Item = new Item

class Client:
  def read(using p: Family)(using i: p.Item): p.Item = i
  def run(using p: Family): p.Item = read
  def direct(using p: Family): p.Item = read(using p)

class Witness
object Witness:
  given derive(using p: Family)(using i: p.Item): Witness = new Witness
class Consumer:
  def needs(using w: Witness): Int = 1
  def conditional(using p: Family): Int = needs
