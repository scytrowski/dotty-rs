package me.cytrowski.tastyfixtures.semantic

class Foo[A](val x: A) {
  def bar[B](b: B): A = x
}
