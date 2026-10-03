object NestedInterpolation:
  def render(name: String, description: String): String =
    s"\n- $name${if description.isEmpty() then "" else s" :\n\t${description.replace("\n", "\n\t")}"}"
