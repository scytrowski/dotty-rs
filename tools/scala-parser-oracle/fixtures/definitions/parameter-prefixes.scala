trait ParameterPrefixParent:
  def overridden: Int

class ParameterPrefixProbe(
    private val privateField: Int,
    protected val protectedField: Int,
    override val overridden: Int,
    val immutableField: Int,
    var mutableField: Int,
    @deprecatedName("old") renamed: Int
) extends ParameterPrefixParent

object ParameterAnnotationProbe:
  def annotated(@deprecatedName("old") value: Int): Int = value
