# Scala 3.9.0 category-five coverage

This matrix records which category-five tags are observed in the checked-in
corpora and which are covered only by focused synthetic Rust tests. The
regression test is `category_five_coverage_matrix_matches_all_real_corpora` in
`tests/scala3_compiler.rs`.

The real corpus consists of the local fixtures plus the pinned
`scala3-library` and `scala3-compiler` baselines. `SUPERtype` and
`APPLYsigpoly` are represented by the dedicated local fixtures
`obscure_tasty/ObscureTasty.tasty` and
`signature_polymorphic/SignaturePolymorphic$package.tasty`.

The complete numeric assignment and all reserved gaps across categories are
also checked by `matches_the_complete_scala_3_9_tag_assignment_matrix` in the
`dotty-tasty` unit suite.

| Tag(s) | Name(s) | Coverage | Notes |
| --- | --- | --- | --- |
| 128–147 | `PACKAGE` … `INLINED` | Real corpus | Baseline and local fixtures |
| 148 | `SELECTouter` | Synthetic only | Compiler-generated outer selection form; not observed in the current ordinary corpus |
| 149–165 | `REPEATED` … `ANDtype` | Real corpus | Baseline and local fixtures |
| 166 | — | Reserved | Unassigned in Scala 3.9.0 |
| 167 | `ORtype` | Real corpus | Baseline corpus |
| 168 | — | Reserved | Unassigned in Scala 3.9.0 |
| 169–173 | `POLYtype` … `ANNOTATION` | Real corpus | Baseline and local fixtures |
| 174 | `TERMREFin` | Synthetic only | No current corpus case; focused parser tests cover the grammar |
| 175–183 | `TYPEREFin` … `SPLICEPATTERN` | Real corpus | Baseline and local fixtures |
| 184–189 | — | Reserved | Unassigned in Scala 3.9.0 |
| 190 | `MATCHtype` | Synthetic only | Rich semantic match-type representation; source syntax normally emits `MATCHtpt`. Decoded to `Type::Match` since Milestone 4d |
| 191 | `MATCHtpt` | Real corpus | Match-type syntax in baseline and local fixtures (27 trees in the library, 0 in the compiler); a tree, not an `unpickle_type` input |
| 192 | `MATCHCASEtype` | Synthetic only | Internal match-case type representation. Decoded to `Type::MatchCase` since Milestone 4d |
| 193 | `FLEXIBLEtype` | Real corpus | Baseline corpus |
| 255 | `HOLE` | Synthetic only | Used for pickled quote trees rather than ordinary TASTy files |

“Synthetic only” does not mean unsupported: every assigned tag remains wired
through structured decoding and encoding, with focused unit coverage. It means
only that the current Scala 3.9.0 `.tasty` corpus does not provide a natural
file-level example for that tag.
