/// Parser-to-scanner feedback events used by Scala 3 layout handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannerEvent {
    ColonEol { in_template: bool },
    Indented,
    Outdented,
    ArrowIndented,
}
