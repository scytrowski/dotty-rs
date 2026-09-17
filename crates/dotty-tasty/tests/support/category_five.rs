#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageKind {
    RealCorpus,
    SyntheticOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageEntry {
    pub name: &'static str,
    pub tag: u8,
    pub kind: CoverageKind,
}

/// Category-five coverage for the Scala 3.9.0 tag matrix.
///
/// `RealCorpus` means that the tag occurs in one of the checked-in local or
/// external Scala 3.9.0 corpora. `SyntheticOnly` means that the Rust parser
/// has focused unit coverage, but no ordinary `.tasty` fixture currently
/// emits the tag.
pub const MATRIX: &[CoverageEntry] = &[
    CoverageEntry {
        name: "PACKAGE",
        tag: 128,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "VALDEF",
        tag: 129,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "DEFDEF",
        tag: 130,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPEDEF",
        tag: 131,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "IMPORT",
        tag: 132,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPEPARAM",
        tag: 133,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "PARAM",
        tag: 134,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "APPLY",
        tag: 136,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPEAPPLY",
        tag: 137,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPED",
        tag: 138,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ASSIGN",
        tag: 139,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "BLOCK",
        tag: 140,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "IF",
        tag: 141,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "LAMBDA",
        tag: 142,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "MATCH",
        tag: 143,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "RETURN",
        tag: 144,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "WHILE",
        tag: 145,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TRY",
        tag: 146,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "INLINED",
        tag: 147,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "SELECTouter",
        tag: 148,
        kind: CoverageKind::SyntheticOnly,
    },
    CoverageEntry {
        name: "REPEATED",
        tag: 149,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "BIND",
        tag: 150,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ALTERNATIVE",
        tag: 151,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "UNAPPLY",
        tag: 152,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ANNOTATEDtype",
        tag: 153,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ANNOTATEDtpt",
        tag: 154,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "CASEDEF",
        tag: 155,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TEMPLATE",
        tag: 156,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "SUPER",
        tag: 157,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "SUPERtype",
        tag: 158,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "REFINEDtype",
        tag: 159,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "REFINEDtpt",
        tag: 160,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "APPLIEDtype",
        tag: 161,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "APPLIEDtpt",
        tag: 162,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPEBOUNDS",
        tag: 163,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPEBOUNDStpt",
        tag: 164,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ANDtype",
        tag: 165,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ORtype",
        tag: 167,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "POLYtype",
        tag: 169,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TYPELAMBDAtype",
        tag: 170,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "LAMBDAtpt",
        tag: 171,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "PARAMtype",
        tag: 172,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "ANNOTATION",
        tag: 173,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "TERMREFin",
        tag: 174,
        kind: CoverageKind::SyntheticOnly,
    },
    CoverageEntry {
        name: "TYPEREFin",
        tag: 175,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "SELECTin",
        tag: 176,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "EXPORT",
        tag: 177,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "QUOTE",
        tag: 178,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "SPLICE",
        tag: 179,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "METHODtype",
        tag: 180,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "APPLYsigpoly",
        tag: 181,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "QUOTEPATTERN",
        tag: 182,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "SPLICEPATTERN",
        tag: 183,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "MATCHtype",
        tag: 190,
        kind: CoverageKind::SyntheticOnly,
    },
    CoverageEntry {
        name: "MATCHtpt",
        tag: 191,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "MATCHCASEtype",
        tag: 192,
        kind: CoverageKind::SyntheticOnly,
    },
    CoverageEntry {
        name: "FLEXIBLEtype",
        tag: 193,
        kind: CoverageKind::RealCorpus,
    },
    CoverageEntry {
        name: "HOLE",
        tag: 255,
        kind: CoverageKind::SyntheticOnly,
    },
];
