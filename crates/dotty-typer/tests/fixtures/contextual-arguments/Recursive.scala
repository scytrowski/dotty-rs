package recursivesearch

trait Evidence
object Evidence:
  implicit def loop(implicit first: => Evidence, second: => Evidence): Evidence = new Evidence {}

class Recursive:
  def read(using ev: Evidence): Evidence = ev
  def derived: Evidence = read
