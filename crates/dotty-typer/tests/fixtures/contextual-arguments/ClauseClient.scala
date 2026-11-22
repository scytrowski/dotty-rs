package clients
import defaults.ClauseReader

class ClauseClient:
  def partial(r: ClauseReader)(using n: Int): Int = r.fold(3)(7)
  def supplied(r: ClauseReader)(using n: Int): Int = r.fold(3)(7, 8)
