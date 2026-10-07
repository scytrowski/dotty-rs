def data_=(data: Data): Unit = {
  val offset =
    if arch == "x86_64" then
      sizeof[UInt]
    else
      sizeof[Ptr[Byte]]
  !(event.asInstanceOf[Ptr[Byte]] + offset).asInstanceOf[Ptr[Data]] = data
}
