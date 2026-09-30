consume(
  values.iterator.map(x =>
    val converted = createEntry(toAbstractFile(x))
    converted
  ).toSeq,
  fallback
)
