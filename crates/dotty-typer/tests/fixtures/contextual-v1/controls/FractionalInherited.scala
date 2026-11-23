package fractionalprobe

trait Slice[T] extends scala.math.Numeric[T]:
  def inherited(v: NumericOps): NumericOps = v
