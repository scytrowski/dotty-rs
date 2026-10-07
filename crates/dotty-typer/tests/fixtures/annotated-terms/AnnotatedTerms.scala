class TermAnnotation extends scala.annotation.StaticAnnotation

object StableTerm

object AnnotatedTerms {
  def literal: Int = 1: @TermAnnotation
  val stableAlias: StableTerm.type = StableTerm
  def stableReference = stableAlias: @TermAnnotation
  def unstableParameter(value: Int): Int = value: @TermAnnotation
  def unstableCall(value: Int) = identity(value): @TermAnnotation
  def nested(value: Int) = (value: @TermAnnotation): @TermAnnotation
}
