package issue2238oracle

import scala.util.{Either, Left, Right}

object GenericUnapply:
  def left[A, B](value: Either[A, B]): Option[A] = value match
    case Left(item)  => Some(item)
    case Right(_)    => None

  def right[A, B](value: Either[A, B]): Option[B] = value match
    case Left(_)     => None
    case Right(item) => Some(item)
