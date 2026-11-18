package dependent

class Family:
  class Item
  type View = Item
  given member: Item = new Item
  def read(using i: Item): Item = i

class Client(val p: Family, val q: Family):
  import p.given
  import q.given
  def left: p.Item = p.read
  def viaView: p.View = p.read
  def right: q.Item = q.read

class Container(val family: Family)
class Nested(val a: Container, val b: Container):
  def deep: a.family.Item = a.family.read
