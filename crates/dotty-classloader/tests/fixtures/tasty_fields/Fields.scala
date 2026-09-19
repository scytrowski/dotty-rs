package me.cytrowski.tastyfixtures.fields

class P(val a: Int, b: Int, var c: Int)

class Q(private val d: Int, protected val e: Int)

class Body(x: Int) {
  val inBody: Int = x
}
