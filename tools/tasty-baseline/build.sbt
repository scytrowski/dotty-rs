ThisBuild / scalaVersion := "3.9.0"

libraryDependencies += "org.scala-lang" %% "scala3-tasty-inspector" % scalaVersion.value

Compile / run / fork := true
