package clients
import defaults.PairReader

class AppliedPairClient:
  def reader(): PairReader = new PairReader
  def direct(): Int = reader().pick(1, 2)(using 9)
  def left(): Int = 13
  def right(): Int = 29
  def fallback(): Int = reader().pick(left(), right())
  def supplied(using n: Int): Int = reader().pick(left(), right())
  def explicit(using a: Int, b: Int): Int = reader().pick(left(), right())(using b)
  def stableChoice(r: PairReader)(using seed: Int): Int = r.choose()
  def appliedChoice(using seed: Int): Int = reader().choose()
  def writtenChoice(using seed: Int): Int = reader().choose(using seed)(7)
  def legacyFallback(): Int = reader().legacy(left(), right())
  def legacyStableFallback(r: PairReader): Int = r.legacy(left(), right())
  def legacySupplied(using n: Int): Int = reader().legacy(left(), right())
  def legacyStableSupplied(r: PairReader)(using n: Int): Int = r.legacy(left(), right())
  def legacyExplicit(using a: Int, b: Int): Int = reader().legacy(left(), right())(b)
  def legacyStableExplicit(r: PairReader)(using a: Int, b: Int): Int = r.legacy(left(), right())(using b)
