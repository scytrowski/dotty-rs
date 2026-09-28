{
  enum FileExtension(val value: String):
    case Tasty extends FileExtension("tasty")
    case Empty extends FileExtension("")

  enum MigrationVersion(val warnFrom: Version, val errorFrom: Version):
    case Scala2to3 extends MigrationVersion(`3.0`, `3.0`)
}
