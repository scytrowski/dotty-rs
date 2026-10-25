import scala.tools.asm.ClassReader
import scala.tools.asm.tree.AbstractInsnNode

object AsmMemberResolution:
  def opcode(node: AbstractInsnNode): Int = node.getOpcode
  def byte(reader: ClassReader, offset: Int): Int = reader.readByte(offset)
  def utf8(reader: ClassReader, offset: Int, buffer: Array[Char]): String =
    reader.readUTF8(offset, buffer)
  def short(reader: ClassReader, offset: Int): Int = reader.readShort(offset)
