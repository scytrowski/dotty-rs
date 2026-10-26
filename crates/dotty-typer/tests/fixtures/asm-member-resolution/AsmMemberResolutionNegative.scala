import scala.tools.asm.tree.AbstractInsnNode

object AsmMemberResolutionNegative:
  def missing(node: AbstractInsnNode): Int = node.noSuchOpcodeMethod()
