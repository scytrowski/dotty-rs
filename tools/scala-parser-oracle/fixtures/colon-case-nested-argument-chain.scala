aliases.find(alt => alt.name == name && alt.deprecation.isDefined)
  .map:
    case SettingAlias(alt, dep) =>
      val msg =
        dep.collect:
          case value if value.nonEmpty => value
        .getOrElse("use the replacement")
      state.warn(s"Option $alt is deprecated: $msg", args)
  .getOrElse(state)
