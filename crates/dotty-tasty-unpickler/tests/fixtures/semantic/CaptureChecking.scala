package me.cytrowski.tastyfixtures.semantic

// The importing file is compiled with capture checking on (the import is
// what enables it, no compiler option). Capturing types are `retains`
// annotations, the only `CompactAnnotation`s Scala 3.9 writes: an
// `ANNOTATEDtype` whose annotation payload is a type, not a tree. An
// inferred type of a local definition is where one is pickled.
import scala.language.experimental.captureChecking

class Cap extends caps.SharedCapability

class CapturingHolder:
  def body(c: Cap^): Int =
    val local = () => c
    local()
    0
