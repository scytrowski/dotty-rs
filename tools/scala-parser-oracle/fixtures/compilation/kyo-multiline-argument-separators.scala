object KyoArgumentLayout:
  def schema = Schema.init(
    writeFn = (value, writer) =>
      value match
        case text: String => writer.string(text)
        case _ => writer.string(value.toString),
    readFn = reader => reader.string()
  )

  def closeAfterMultilineArgument = consume(
    value match
      case Some(result) => result
      case None => fallback
  )

  def keepNewlineBetweenStatements =
    consume(value)
    nextStatement
