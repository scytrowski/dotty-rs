class Annot(val n: Int) extends scala.annotation.StaticAnnotation
class EmptyAnnot extends scala.annotation.StaticAnnotation
class Multi(val first: Int, val second: Int) extends scala.annotation.StaticAnnotation

object SourceAnnotations {
  def bare(value: Int): Int = value: @unchecked
  def empty(value: Int): Int = value: @EmptyAnnot()
  def positional(value: Int): Int = value: @Annot(1)
  def named(value: Int): Int = value: @Annot(n = 1)
  def mixed(value: Int): Int = value: @Multi(1, second = 2)
}
