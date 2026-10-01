object LocalValCatchCase {
  def compile(files: List[Int]) =
    try compileSources(files.map(identity))
    catch case ex: Exception if !this.enrichedErrorMessage =>
      val files1 = if units.isEmpty then files else units.map(_.source.file)
      report.echo(files1.map(_.path))
      throw ex
}
