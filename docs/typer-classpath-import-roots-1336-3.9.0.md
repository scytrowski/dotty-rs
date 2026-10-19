# Unresolved import-root resource scan

This report scans the frozen #1180 rows against the explicitly supplied, ordered JAR list. A `.class` or `.tasty` hit is resource evidence only; this scan does not prove a Scala `object`, `ModuleClass`, member completion, or selected import.

The row-level evidence is in [`typer-classpath-import-roots-1336-3.9.0.tsv`](typer-classpath-import-roots-1336-3.9.0.tsv), the deduplicated lexical candidate matrix is in [`typer-classpath-import-roots-1336-target-candidates.tsv`](typer-classpath-import-roots-1336-target-candidates.tsv), resolver outcomes are in [`typer-classpath-import-roots-1336-resolver-results.tsv`](typer-classpath-import-roots-1336-resolver-results.tsv), source declaration owners are in [`typer-classpath-import-roots-1336-target-sources.tsv`](typer-classpath-import-roots-1336-target-sources.tsv), and object/version notes are in [`typer-classpath-import-root-fixtures-1336-3.9.0.md`](typer-classpath-import-root-fixtures-1336-3.9.0.md). Historical blocker and selector evidence remains in the unchanged [#1180 reconciliation](typer-classpath-import-prefix-profile-1180-3.9.0-reconciliation.tsv).

Scala: 3.9.0; JDK: 21.0.8+9.

## Pinned archives

| order | archive | SHA-256 |
|---:|---|---|
| 1 | `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| 2 | `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| 3 | `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| 4 | `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| 5 | `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| 6 | `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| 7 | `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| 8 | `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |
| 9 | `java.base.jmod` | `7e05214a446061c5543f3732f653f514d4e05fa6fd2545aa792c205b24362791` |
| 10 | `java.compiler.jmod` | `ebd424405bb9c97e8b09c9f0573eda15e2c80f299cecbefe051077e41961a544` |
| 11 | `java.datatransfer.jmod` | `50fe810eccddd61c31720f1512564a57951fc120dd3392f7c99e7fa8ff30b891` |
| 12 | `java.desktop.jmod` | `6bda861a1f6de1d7b6ffcff2f4845bf4d92a6d26c907b37c335b584b8ab790e3` |
| 13 | `java.instrument.jmod` | `c22c7ee4729d32bc29434e3e53fa475f73d8f34a417ee9add957d89417ceb3b9` |
| 14 | `java.logging.jmod` | `63eec265d767becc0f6a8adba1a8c989046c8c46c75ae161169ca666feb13f95` |
| 15 | `java.management.jmod` | `c65fd9b559d0726ca450e62aa39c2fccf26100ef4a975c830299ab899c5c2361` |
| 16 | `java.management.rmi.jmod` | `57a41a0fa74b60687215c3f90c62e19c6b1fd673eae932edf04a263d68a4ffa4` |
| 17 | `java.naming.jmod` | `3eb84f3cdef0107b08e55f462b69c98ad357e36a10cdb60cc473816504a935eb` |
| 18 | `java.net.http.jmod` | `ab7d3ebf18a8d3c5dc3eb272f298d1da0ecb25b44f8c141af4dcfd65c9215287` |
| 19 | `java.prefs.jmod` | `267eb0db6866acae8eedabf59a0c14ef6f24b6c257c3c6585de34e55f98c9f82` |
| 20 | `java.rmi.jmod` | `e0352d16cf3e69434a8f173beb7b16bf70095b87e8dfe1abd6e3212b5d8fdf52` |
| 21 | `java.scripting.jmod` | `f805012220bd244db69761e803b6f7901dff62fa7f3ec288d2f5f3be98e4c03a` |
| 22 | `java.se.jmod` | `dc9098033ba454929cfcbdb1e3a947340294f73a5078c2c03ec406d3d06f3ec8` |
| 23 | `java.security.jgss.jmod` | `cfef7b329b128923a007ce41683b6a607498e831e0bd0a10b75c3b797f139261` |
| 24 | `java.security.sasl.jmod` | `bc5c91166b2f7263679a8bb26b6fd7c78cd57097dbffe66cb603a29baea9156d` |
| 25 | `java.smartcardio.jmod` | `85cb022dd91c09780136fcb931add84203c9ed20c637c79da2e30a5f8a86ec95` |
| 26 | `java.sql.jmod` | `f94bb15cc32777bcb28089cebeca714fb1e3170fbeacb63364199f1e601b8625` |
| 27 | `java.sql.rowset.jmod` | `ffe47f7b45b31adf77e4f7c908cde6627333311e012f16d8c6c75f263ef1da46` |
| 28 | `java.transaction.xa.jmod` | `cc23a12ece6cb8a7a5a6b2413db478f8b989d74043898f4449fcd0d71cae0458` |
| 29 | `java.xml.crypto.jmod` | `62ab4714f3171d194f6c792e456a978d5eacb0c8132ae998302e9bad833e1e7a` |
| 30 | `java.xml.jmod` | `2545f0f6aa971e43d67e1780d6147c7b68d833ef9a8d74c6d3cc9cf98e89a09c` |
| 31 | `jdk.accessibility.jmod` | `cc174fdafeac1a7224e60c4f8f0ec13052b68bf5b736f96b3852baeff271bb5e` |
| 32 | `jdk.attach.jmod` | `aa6cd83b2387340087c0b044ff8a94fafeafba53adc23095a06f505a3cb3aac8` |
| 33 | `jdk.charsets.jmod` | `b2896dfa32fe990689698c82ba533f87d6a2eedd4f061a3673aefa3cd50554dd` |
| 34 | `jdk.compiler.jmod` | `ffba2b8d701a8c3f54909ad2dd9bb1b2448cd61d80199da1eeabbd14eff0a3ed` |
| 35 | `jdk.crypto.cryptoki.jmod` | `e7be2c98024bbd3794ba586e5616eef8d9dcf97106e8c92b1bedb96e44336bd3` |
| 36 | `jdk.crypto.ec.jmod` | `a9304a84565dc8389aaefaab50bd508cbabd147e64c445bbbdaf646fab8113fc` |
| 37 | `jdk.dynalink.jmod` | `7b9bad2b51f479a1f5025c89c23950ad7df66ae974ebd3c5bfbd2b00d3c8c93b` |
| 38 | `jdk.editpad.jmod` | `237e00e4e5ea08fb638f615e6102f25b8c0a0dc164cf3a252dc1b76ad0d2419d` |
| 39 | `jdk.hotspot.agent.jmod` | `562257f105f7bc8e6d2d7538372ea02798a3f5b4fcf0bad7731c6631e2aa53b7` |
| 40 | `jdk.httpserver.jmod` | `ce85319bc9d25875d7bbec6833bdf1986d92559c08d4821be0396db1fe950158` |
| 41 | `jdk.incubator.vector.jmod` | `e4cd0ac1c187deefbf1ce8188a3009c3d11cfab67e60254604bd5ba2439ede30` |
| 42 | `jdk.internal.ed.jmod` | `36beab84d43b17243cf8777658607a86f57efd7317767dca68f9cbabbd3821b2` |
| 43 | `jdk.internal.jvmstat.jmod` | `7900076e22407be35f96aeb61e338c4ebb26e8eda1fe3a28edd8a50db6b56cb0` |
| 44 | `jdk.internal.le.jmod` | `d7b5ba53c91a869eae0a5192d9e1bf6a68e69f447ad0fb47f738235b48b5225e` |
| 45 | `jdk.internal.opt.jmod` | `61f95c9016986b437d1d61303095ff43cd9f9b77be92710f634c8d09011c3c15` |
| 46 | `jdk.internal.vm.ci.jmod` | `09d82bb46d0009cd57d0a774d76d021e5a3f64653f064f8e290fea6f42363211` |
| 47 | `jdk.internal.vm.compiler.jmod` | `761660ce3b633075c7480db813f842a1308654cf964c6425efb6307a62fd970a` |
| 48 | `jdk.internal.vm.compiler.management.jmod` | `c5cd5ffe0527da0fb1826fa8678db8cef3c37d03c908e36011349f001e602d27` |
| 49 | `jdk.jartool.jmod` | `c840cc8f8fc1765ce24d76ab8976aac32dd742f73e831f2d048d8fead0b1e723` |
| 50 | `jdk.javadoc.jmod` | `27251e37fc116c1fcd4d4015cc018e06abe46df9ed4ec1d2f530d6837267101e` |
| 51 | `jdk.jcmd.jmod` | `a515c6bf727219144e49728aacd52a5a4ce67bbd136430bea83ef8f11ad298a4` |
| 52 | `jdk.jconsole.jmod` | `25cbbbde4a93341db48acadc5a06f3c8c7ba81d829573895069b97cef7ce4672` |
| 53 | `jdk.jdeps.jmod` | `2abbbee9cd65f8e4116f02794a8584f1ae9ab287305c23f40541ec1825fb4c09` |
| 54 | `jdk.jdi.jmod` | `b688a694b7b1806f4d6eba45b6ce980e1b8a9edb4b28f1d6cb039345cfa6fd9c` |
| 55 | `jdk.jdwp.agent.jmod` | `f50c33b639cf982a4ac7df2d26d4f8b92c7412575a6e82a513f46c87f4e0c67f` |
| 56 | `jdk.jfr.jmod` | `5d613a880aaa02dfc299c71dc4b7bec094a15202928d5e1fd1f5cc82a2ddc72b` |
| 57 | `jdk.jlink.jmod` | `f184a1d01ec0134eb22d02c1b68a5ac6cd1818cd9dc5f1d39e2ce9f79ecf8625` |
| 58 | `jdk.jpackage.jmod` | `2e66ae437e37fe6100d1ddec855def6eb78ad05ca8522cc147f20ec70486ed51` |
| 59 | `jdk.jshell.jmod` | `bb1d942f844d6c92eb6e800630528f53beb5b7b5e1bb93a6082c959adbb91899` |
| 60 | `jdk.jsobject.jmod` | `b2c4da5e5d9f441745ed6ff51beed1f1c0b10c24b8b07747090c9b0841cb40e5` |
| 61 | `jdk.jstatd.jmod` | `50bb6e7590b13a02b469ec0b77d0b57e8572dca0d94b0a2eb8f512cca52f0be6` |
| 62 | `jdk.localedata.jmod` | `14a8ec30ee75f20b986fb0df98de5e7fc414506fc068c0df2c0f7a9379eb89bc` |
| 63 | `jdk.management.agent.jmod` | `e7df1dc7ac2af136e4723a6eb6e18f2f66d09fc1638fce6a0e12259eabfdbac2` |
| 64 | `jdk.management.jfr.jmod` | `66b9c33910b434d34c3afcadcfe5f86de0f08704bf8abf19ee300f1487026ada` |
| 65 | `jdk.management.jmod` | `5c54f3cf04d85124b224b42579da0093b0aa2e6763abd9d6eebe82d90b63828e` |
| 66 | `jdk.naming.dns.jmod` | `d4bac900910e901d122a7adc58de3abca9e08d697ced6c81d0263fe246bd4c39` |
| 67 | `jdk.naming.rmi.jmod` | `cb3f1b0efe9f1dec3112bf3ffc7a0c6acbdf946b07745b925ea085f0f89da8aa` |
| 68 | `jdk.net.jmod` | `cf64298f9d2339a7740770c61708d5584c5986debe3b7ebda8e532b4736cd5d3` |
| 69 | `jdk.nio.mapmode.jmod` | `cc6128230b5b915fcef2226de9c6086f865d19c8b152f0b8e0e0c19627f37217` |
| 70 | `jdk.random.jmod` | `1888f42044a8d8f3d0553c46783dfce22bbe5b2357b62f0a2d11b376d880be8f` |
| 71 | `jdk.sctp.jmod` | `d2ea6d655846f66c686dbbd2211fd55fa288283182d1366420ff58be58548e41` |
| 72 | `jdk.security.auth.jmod` | `1ff1c7794ac49541a7977ce34dd9b5ae5e78448755bf8ecef231679d206132c9` |
| 73 | `jdk.security.jgss.jmod` | `69f6e55734f467c96ffc53193e459ab1eea8e0ae9db3187d9f73e10fe56fd1fe` |
| 74 | `jdk.unsupported.desktop.jmod` | `61a8fc10520973319b0d41f31b8c54553516e438ca0a9b47342ebf619f5eb4a5` |
| 75 | `jdk.unsupported.jmod` | `54013b005781ab2dd24511972460a8d7d745aff32a6ca277ac9ff3e4e0aac431` |
| 76 | `jdk.xml.dom.jmod` | `26fc071fa3c598e5ae9def71c4ca65fb4bb11db550a14822374b4c800cf22260` |
| 77 | `jdk.zipfs.jmod` | `36729c04a3a7a1d7f37265abc43fc44b307a081452f870a0f5196ab4e5b39b5e` |

## Frozen cohort

Observations: 1063; files: 81; target keys: 31; direct blocker origins: 0; inherited blocker origins: 393.

## Resource classifications

- `candidate_resource_present_identity_requires_semantic_oracle`: 1055 observations
- `no_candidate_resource_in_supplied_classpath`: 8 observations

## Target matrix

| target | observations | resource result | earliest evidence |
|---|---:|---|---|
| `Action::<first-segment-unresolved>` | 1 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/reporting/Action.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Character::<first-segment-unresolved>` | 14 | candidate_resource_present_identity_requires_semantic_oracle | `classes/java/lang/Character.class` from `java.base.jmod` (TASTy -) |
| `Contexts::<first-segment-unresolved>` | 167 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Contexts.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Decorators::<first-segment-unresolved>` | 13 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Decorators.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Diagnostic::<first-segment-unresolved>` | 5 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/reporting/Diagnostic.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `FileUtils::<first-segment-unresolved>` | 1 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/classpath/FileUtils.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Flags::<first-segment-unresolved>` | 2 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Flags.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `INodeBase::<first-segment-unresolved>` | 3 | candidate_resource_present_identity_requires_semantic_oracle | `scala/collection/concurrent/INodeBase.class` from `scala-library-3.9.0.jar` (TASTy -) |
| `JavaTokens::<first-segment-unresolved>` | 18 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/parsing/JavaTokens.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Manifest::<first-segment-unresolved>` | 1 | candidate_resource_present_identity_requires_semantic_oracle | `scala/reflect/Manifest.tasty` from `scala-library-3.9.0.jar` (TASTy 28.9.0) |
| `MegaPhase::<first-segment-unresolved>` | 69 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/transform/MegaPhase.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `NameKinds::<first-segment-unresolved>` | 1 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/NameKinds.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Names::<first-segment-unresolved>` | 5 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Names.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Parsers::<first-segment-unresolved>` | 3 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/parsing/Parsers.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Periods::<first-segment-unresolved>` | 58 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Periods.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `ProtoTypes::<first-segment-unresolved>` | 179 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/typer/ProtoTypes.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Scanners::<first-segment-unresolved>` | 5 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/parsing/Scanners.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Settings::<first-segment-unresolved>` | 12 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/config/Settings.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `SymDenotations::<first-segment-unresolved>` | 1 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/SymDenotations.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `SymbolInformation::<first-segment-unresolved>` | 3 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/semanticdb/SymbolInformation.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Symbols::<first-segment-unresolved>` | 80 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Symbols.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Texts::<first-segment-unresolved>` | 55 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/printing/Texts.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Tokens::<first-segment-unresolved>` | 116 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/parsing/Tokens.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Trees::<first-segment-unresolved>` | 17 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/ast/Trees.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `Types::<first-segment-unresolved>` | 215 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/core/Types.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `ValueClasses::<first-segment-unresolved>` | 2 | candidate_resource_present_identity_requires_semantic_oracle | `dotty/tools/dotc/transform/ValueClasses.tasty` from `scala3-compiler_3-3.9.0.jar` (TASTy 28.9.0) |
| `collection::<first-segment-unresolved>` | 2 | no_candidate_resource_in_supplied_classpath | - |
| `core::<first-segment-unresolved>` | 1 | no_candidate_resource_in_supplied_classpath | - |
| `num::<first-segment-unresolved>` | 2 | no_candidate_resource_in_supplied_classpath | - |
| `processInternal::<first-segment-unresolved>` | 9 | candidate_resource_present_identity_requires_semantic_oracle | `scala/sys/process/processInternal.tasty` from `scala-library-3.9.0.jar` (TASTy 28.9.0) |
| `quotes::<first-segment-unresolved>` | 3 | no_candidate_resource_in_supplied_classpath | - |

The row-level TSV preserves every frozen row. `ordered_candidate_evidence` records each direct class/TASTy resource hit in lexical-candidate order, including archive digest and parsed TASTy header. No historical source or selector outcome is rewritten. The report does not count Scala ModuleClass requests, completions, or selected members; those must come from a resolver-level audit, not inferred from archive names.

## Resolver summary

The ignored serial resolver audit issued 118 Term and 118 Type `MemberRequest`s across the deduplicated lexical candidates, plus 118 full package-path checks and five selected-member probes. Term results: 25 Objects, one Package, 92 missing, no errors. Type results: three Classes, one Trait, 91 missing, 23 typed resolver errors. Exactly one full candidate package path exists (`scala.collection`). The member and per-candidate details are recorded in the resolver TSV.
