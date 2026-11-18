class PlainConstructorParam(sequenceOfChars: IndexedSeq[Char]) {
  def charAt(index: Int): Char = sequenceOfChars(index)
}

class ValConstructorParam(val sequenceOfChars: IndexedSeq[Char]) {
  def charAt(index: Int): Char = sequenceOfChars(index)
}

class ShadowedConstructorParam(sequenceOfChars: IndexedSeq[Char]) {
  def charAt(sequenceOfChars: Array[Char]): Char = sequenceOfChars(0)
}

object IndexedSeqApplyControl {
  def applyChar(sequenceOfChars: IndexedSeq[Char], index: Int): Char = sequenceOfChars(index)
}
