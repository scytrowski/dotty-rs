package strictsearch

trait Evidence
object Evidence:
  implicit def loop(implicit again: Evidence): Evidence = again

class Strict:
  def read(using ev: Evidence): Evidence = ev
  def derived: Evidence = read
