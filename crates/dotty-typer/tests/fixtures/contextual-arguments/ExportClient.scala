package exportsprint
class Client:
  def need[A](using e: TC[A]): TC[A] = e
  def direct: TC[Int] = { import Instances.given; need[Int] }
  def named: TC[Int] = Named.ints
  def renamed: TC[Int] = { import Alias.given; need[Int] }
  def aliasSelection: TC[Int] = Alias.selected
  def wildcard: TC[Boolean] = { import All.given; need[Boolean] }
