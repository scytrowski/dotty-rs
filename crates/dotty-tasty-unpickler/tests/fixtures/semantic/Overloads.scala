package me.cytrowski.tastyfixtures.semantic

class Overloads {
  def f(x: Int): Int = x
  def f(x: String): String = x
}

class Plain(y: Int)
