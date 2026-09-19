ThisBuild / scalaVersion := "3.9.0"

libraryDependencies += "org.scala-lang" %% "scala3-compiler" % scalaVersion.value

Compile / run / fork := true
