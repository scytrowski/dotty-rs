object LeadingOperator:
  def check =
    first
    || second
    || nested
        && third
            .exists(ready)
      // The infix chain continues after comment-only lines.
      // Its operator aligns with the surrounding expression.
    || fallback
  def compare =
       first
    && second
