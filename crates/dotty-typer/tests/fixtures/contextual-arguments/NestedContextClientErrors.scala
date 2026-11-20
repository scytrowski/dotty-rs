package clients

abstract class NestedContextClientErrors:
  def build: Int ?=> Boolean ?=> Int
  def missing(using n: Int): Int = build
  def ambiguous(using n: Int, left: Boolean, right: Boolean): Int = build
