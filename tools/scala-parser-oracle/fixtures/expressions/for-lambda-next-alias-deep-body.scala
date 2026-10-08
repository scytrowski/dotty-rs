for
  value <- values
  transform = (item: Int) =>
    val incremented = item + 1
    if incremented > 1 then
      val doubled = incremented * 2
      doubled
    else
      item
  result = transform(value)
  last <- values
yield result
