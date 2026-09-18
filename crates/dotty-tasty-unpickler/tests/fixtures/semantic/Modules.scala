package me.cytrowski.tastyfixtures.semantic

trait Marker {
  def mark: Int
}

class Both

object Both {
  def make: Both = new Both
}
