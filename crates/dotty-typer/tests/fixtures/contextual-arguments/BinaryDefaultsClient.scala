package clients
import defaults.Reader

class DefaultClient:
  def next(): Int = 7
  def fallback(reader: Reader): Int = reader.offset(next())
  def supplied(reader: Reader)(using chosen: Int): Int = reader.offset(next())
  def explicit(reader: Reader): Int = reader.offset(7)(using 9)
  def noEarlierClause(reader: Reader): Int = reader.read
