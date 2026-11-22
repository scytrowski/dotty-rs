package exportsprint
class Client:
  def need[A](using e: TC[A]): TC[A] = e
  def direct: TC[Int] = { import Instances.given; need[Int] }
  def named: TC[Int] = Named.ints
