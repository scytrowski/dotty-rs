object ScalaConsumer {
 def main(args: Array[String]): Unit = {
  val p = new example.BooleanLocals()
  assert(p.declared(true) && !p.declared(false))
  assert(p.mutable(false) && !p.mutable(true))
  assert(p.loop(true) == 1 && p.loop(false) == 0)
  assert(p.effects(true) && !p.effects(false))
  assert(p.guardEffects(false, 1) == 10 && p.guardEffects(true, 1) == 30)
  assert(p.tailFormal(true, false) && !p.tailFormal(false, true))
  assert(p.tailLocal(true, false) && !p.tailLocal(false, true))
  assert(p.tailPattern(false))
  assert(p.effectsOr(false) && !p.effectsOr(true))
  assert(!p.tailSingleton(true) && p.tailSingleton(false))
  assert(!p.tailConstant())
  assert(p.constantLoop() == 1 && p.falseLoop() == 2)
  assert(p.siblingScopes(true) == 1 && p.siblingScopes(false) == 4)
  assert(!p.parameterShadow(true))
  assert(p.bothReturn(true, false) && !p.bothReturn(false, true))
  p.unit(true)
  println("boolean local oracle ok")
 }
}
