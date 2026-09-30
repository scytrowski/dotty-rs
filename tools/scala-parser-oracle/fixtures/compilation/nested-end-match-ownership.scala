object NestedEndMatch:
  def run = {
    if ready then
      value match
        case Some(x) => consume(x)
        case None => fallback()
      end match
    end if
    after()
  }
end NestedEndMatch
