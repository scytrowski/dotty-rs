package patherrors

class Family:
  class Item
  given member: Item = new Item
  def read(using i: Item): Item = i

class Client(val p: Family, val q: Family):
  import p.given
  def wrong: p.Item = q.read
