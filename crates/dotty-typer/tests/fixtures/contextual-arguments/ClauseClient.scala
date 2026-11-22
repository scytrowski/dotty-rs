package clients
import defaults.ClauseReader

class ClauseClient:
  def partial(r: ClauseReader)(using n: Int): Int = r.fold(3)(7)
  def supplied(r: ClauseReader)(using n: Int): Int = r.fold(3)(7, 8)
  def both(r: ClauseReader)(using n: Int): Int = r.fold(3)()
  def reader(): ClauseReader = new ClauseReader
  def seed(): Int = 2
  def next(): Int = 1
  def effects(): Int = reader().fold(using seed())(next())()
  def empty(r: ClauseReader)(using n: Int, flag: Boolean): Int = r.finish(3)()()
  def legacyFallback(): Int = reader().legacy(next())(next())
  def legacySupplied(r: ClauseReader)(using n: Int): Int = r.legacy(1)(2)
  def legacyExplicit(r: ClauseReader)(using a: Int, b: Int): Int = r.legacy(1)(2)(using b)
