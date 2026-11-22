package clients
import defaults.ClauseReader

class ClauseErrors:
  def reader(): ClauseReader = new ClauseReader
  def next(): Int = 1
  def missing(r: ClauseReader): Int = r.fold(3)()
  def ambiguous(r: ClauseReader)(using a: Int, b: Int): Int = r.fold(3)()
  def late(r: ClauseReader)(using n: Int)(using a: Boolean, b: Boolean): Int = r.finish(3)()()
  def lateEffects(using n: Int)(using a: Boolean, b: Boolean): Int = reader().finish(next())()()
  def legacyAmbiguous(r: ClauseReader)(using a: Int, b: Int): Int = r.legacy(1)(2)
