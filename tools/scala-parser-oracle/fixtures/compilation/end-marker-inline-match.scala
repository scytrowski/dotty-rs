object O:
  inline def render(value: Int) =
    inline value match
      case _ => value
    end match
  def after = true
