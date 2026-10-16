//! Opt-in integration check against an installed JDK runtime image.
//!
//! Run with `DOTTY_JDK_JMODS=/path/to/jdk/jmods cargo test -p
//! dotty-classloader --test real_jdk_class_path -- --ignored`. If
//! `DOTTY_JDK_JMODS` is unset, the test requires `JAVA_HOME` and reads its
//! `jmods` directory. It is ignored in normal CI and never downloads a JDK.

use std::path::{Path, PathBuf};
use std::process::Command;

use dotty_classfile::access_flags::ACC_MODULE;
use dotty_classfile::attribute::{Attribute, MODULE_ACC_MANDATED, MODULE_ACC_TRANSITIVE};
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassOrigin, ClassPathEntry, JdkClassPath, JdkModuleDescriptor,
};

fn jmods_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("DOTTY_JDK_JMODS") {
        return PathBuf::from(path);
    }
    PathBuf::from(
        std::env::var_os("JAVA_HOME")
            .expect("set DOTTY_JDK_JMODS or JAVA_HOME to run the real-JDK classpath gate"),
    )
    .join("jmods")
}

fn jmod_executable(jmods: &Path, java_home: Option<&Path>) -> Result<PathBuf, String> {
    let executable = if cfg!(windows) { "jmod.exe" } else { "jmod" };
    let candidate = jmods.parent().unwrap_or(jmods).join("bin").join(executable);
    if candidate.is_file() {
        return Ok(candidate);
    }

    if let Some(home) = java_home
        && let (Ok(selected_jmods), Ok(home_jmods)) =
            (jmods.canonicalize(), home.join("jmods").canonicalize())
        && selected_jmods == home_jmods
    {
        let candidate = home.join("bin").join(executable);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "matching JDK jmod tool was not found beside {}; JAVA_HOME/PATH fallbacks are ignored because they may belong to a different JDK",
        jmods.display()
    ))
}

#[test]
fn jmod_cli_stays_with_the_selected_jmods_despite_a_different_or_invalid_java_home() {
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/jmod-tool-selection-{}",
        std::process::id()
    ));
    let selected_home = scratch.join("selected-jdk");
    let selected_jmods = selected_home.join("jmods");
    let selected_tool =
        selected_home
            .join("bin")
            .join(if cfg!(windows) { "jmod.exe" } else { "jmod" });
    let other_home = scratch.join("other-jdk");
    let other_tool = other_home
        .join("bin")
        .join(if cfg!(windows) { "jmod.exe" } else { "jmod" });
    let missing_home = scratch.join("missing-jdk");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&selected_jmods).unwrap();
    std::fs::create_dir_all(selected_tool.parent().unwrap()).unwrap();
    std::fs::create_dir_all(other_tool.parent().unwrap()).unwrap();
    std::fs::write(&selected_tool, b"selected JDK tool").unwrap();
    std::fs::write(&other_tool, b"unrelated JDK tool").unwrap();

    assert_eq!(
        jmod_executable(&selected_jmods, Some(&other_home)).unwrap(),
        selected_tool,
        "an unrelated JAVA_HOME must not override an explicit JMOD path"
    );
    assert_eq!(
        jmod_executable(&selected_jmods, Some(&missing_home)).unwrap(),
        selected_tool,
        "a stale JAVA_HOME must not override an explicit JMOD path"
    );

    let detached_jmods = scratch.join("detached/jmods");
    std::fs::create_dir_all(&detached_jmods).unwrap();
    assert!(
        jmod_executable(&detached_jmods, Some(&other_home)).is_err(),
        "without the matching tool, do not silently fall back to an unrelated JAVA_HOME"
    );
    let _ = std::fs::remove_dir_all(scratch);
}

fn run_tool(executable: &Path, args: &[&str]) -> String {
    let output = Command::new(executable)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not run {} {args:?}: {error}", executable.display()));
    assert!(
        output.status.success(),
        "{} {args:?} failed: {}",
        executable.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("jmod output is UTF-8")
}

fn run_jmod(jmod: &Path, args: &[&str]) -> String {
    run_tool(jmod, args)
}

fn jmod_observable_lines(module: &JdkModuleDescriptor) -> Vec<String> {
    let mut rows = vec![format!(
        "{}@{}",
        module.name,
        module.version.as_deref().expect("JDK module version")
    )];
    rows.extend(module.requires.iter().map(|row| {
        let mut line = format!("requires {}", row.module);
        if row.flags & MODULE_ACC_TRANSITIVE != 0 {
            line.push_str(" transitive");
        }
        if row.flags & MODULE_ACC_MANDATED != 0 {
            line.push_str(" mandated");
        }
        line
    }));
    rows.extend(module.exports.iter().map(|row| {
        if row.targets.is_empty() {
            format!("exports {}", row.package)
        } else {
            format!(
                "qualified exports {} to {}",
                row.package,
                row.targets.join(" ")
            )
        }
    }));
    rows.extend(module.opens.iter().map(|row| {
        if row.targets.is_empty() {
            format!("opens {}", row.package)
        } else {
            format!(
                "qualified opens {} to {}",
                row.package,
                row.targets.join(" ")
            )
        }
    }));
    rows.extend(module.uses.iter().map(|service| format!("uses {service}")));
    rows.extend(module.provides.iter().map(|row| {
        let mut providers = row.providers.clone();
        providers.sort();
        format!("provides {} with {}", row.service, providers.join(" "))
    }));
    if let Some(main_class) = &module.main_class {
        rows.push(format!("main-class {main_class}"));
    }
    rows.sort();
    rows
}

fn jmod_listed_packages(
    jmod: &Path,
    jmod_path: &Path,
    module: &JdkModuleDescriptor,
) -> Vec<String> {
    let listing = run_jmod(jmod, &["list", jmod_path.to_str().unwrap()]);
    let exported = module
        .exports
        .iter()
        .map(|row| row.package.as_str())
        .collect::<std::collections::HashSet<_>>();
    let packages = listing
        .lines()
        .filter_map(|entry| {
            let class = entry.strip_prefix("classes/")?;
            if class == "module-info.class" || class.starts_with("META-INF/") {
                return None;
            }
            let package = class.rsplit_once('/')?.0.replace('/', ".");
            (!exported.contains(package.as_str())).then_some(package)
        })
        .collect::<std::collections::BTreeSet<_>>();
    packages
        .into_iter()
        .map(|package| format!("contains {package}"))
        .collect()
}

fn assert_raw_descriptor_matches_catalog(
    descriptor: &ClassFile<'_>,
    catalog: &JdkModuleDescriptor,
) {
    assert_eq!(descriptor.access_flags.0, ACC_MODULE);
    descriptor
        .validate_module_descriptor()
        .expect("JDK module descriptor has valid ACC_MODULE shape and typed metadata");
    let module = descriptor
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Module(module) => Some(module),
            _ => None,
        })
        .expect("Module attribute is present");
    let pool = &descriptor.constant_pool;
    assert_eq!(
        pool.module_name(module.module_name_index).unwrap(),
        catalog.name
    );
    assert_eq!(
        module
            .module_version_index
            .map(|index| pool.utf8(index).unwrap()),
        catalog.version.as_deref()
    );
    assert_eq!(module.module_flags, catalog.flags);
    assert_eq!(module.requires.len(), catalog.requires.len());
    for (raw, projected) in module.requires.iter().zip(&catalog.requires) {
        assert_eq!(
            pool.module_name(raw.requires_index).unwrap(),
            projected.module
        );
        assert_eq!(raw.requires_flags, projected.flags);
        assert_eq!(
            raw.requires_version_index
                .map(|index| pool.utf8(index).unwrap()),
            projected.version.as_deref()
        );
    }
    assert_eq!(module.exports.len(), catalog.exports.len());
    for (raw, projected) in module.exports.iter().zip(&catalog.exports) {
        assert_eq!(
            pool.package_name(raw.exports_index)
                .unwrap()
                .replace('/', "."),
            projected.package
        );
        assert_eq!(raw.exports_flags, projected.flags);
        assert_eq!(
            raw.exports_to
                .iter()
                .map(|index| pool.module_name(*index).unwrap())
                .collect::<Vec<_>>(),
            projected
                .targets
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(module.opens.len(), catalog.opens.len());
    for (raw, projected) in module.opens.iter().zip(&catalog.opens) {
        assert_eq!(
            pool.package_name(raw.opens_index)
                .unwrap()
                .replace('/', "."),
            projected.package
        );
        assert_eq!(raw.opens_flags, projected.flags);
        assert_eq!(
            raw.opens_to
                .iter()
                .map(|index| pool.module_name(*index).unwrap())
                .collect::<Vec<_>>(),
            projected
                .targets
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        module
            .uses
            .iter()
            .map(|index| pool.class_name(*index).unwrap().replace('/', "."))
            .collect::<Vec<_>>(),
        catalog.uses
    );
    assert_eq!(module.provides.len(), catalog.provides.len());
    for (raw, projected) in module.provides.iter().zip(&catalog.provides) {
        assert_eq!(
            pool.class_name(raw.provides_index)
                .unwrap()
                .replace('/', "."),
            projected.service
        );
        assert_eq!(
            raw.provides_with
                .iter()
                .map(|index| pool.class_name(*index).unwrap().replace('/', "."))
                .collect::<Vec<_>>(),
            projected.providers
        );
    }
    let packages = descriptor
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::ModulePackages(packages) => Some(
                packages
                    .package_indices
                    .iter()
                    .map(|index| pool.package_name(*index).unwrap().replace('/', "."))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
    assert_eq!(packages, catalog.packages);
    let main_class = descriptor
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::ModuleMainClass(index) => {
                Some(pool.class_name(*index).unwrap().replace('/', "."))
            }
            _ => None,
        });
    assert_eq!(main_class.as_deref(), catalog.main_class.as_deref());
}

fn assert_real_class(class_path: &JdkClassPath, jmods: &Path, module: &str, internal_name: &str) {
    let resource = class_path
        .find_class(&BinaryName::from_internal(internal_name))
        .unwrap_or_else(|error| panic!("lookup for {internal_name} failed: {error}"))
        .unwrap_or_else(|| panic!("{internal_name} was absent from the supplied JDK"));

    assert_eq!(resource.format(), ClassFormat::Class);
    assert_eq!(
        resource.origin(),
        &ClassOrigin::Jmod(jmods.join(format!("{module}.jmod")))
    );

    let mut reader = Reader::new(resource.bytes());
    let class_file = ClassFile::decode(&mut reader)
        .unwrap_or_else(|error| panic!("{internal_name} did not decode: {error}"));
    assert_eq!(
        class_file
            .constant_pool
            .class_name(class_file.this_class)
            .unwrap(),
        internal_name
    );
    assert!(
        class_file.version.major >= 65,
        "expected JDK 21+ class version, got {}",
        class_file.version.major
    );
}

#[test]
#[ignore = "requires an installed JDK 21+; see docs/classloader.md"]
fn finds_and_decodes_classes_from_real_jdk_modules() {
    let jmods = jmods_directory()
        .canonicalize()
        .expect("the supplied JDK jmods directory must exist");
    let class_path = JdkClassPath::new(&jmods).expect("the supplied JMOD directory must open");

    assert_real_class(&class_path, &jmods, "java.base", "java/lang/Object");
    assert_real_class(&class_path, &jmods, "java.base", "java/lang/String");
    assert_real_class(
        &class_path,
        &jmods,
        "java.net.http",
        "java/net/http/HttpClient",
    );
    assert!(
        class_path
            .find_class(&BinaryName::from_internal("dotty/missing/NoSuchClass"))
            .unwrap()
            .is_none()
    );
}

#[test]
#[ignore = "requires an installed JDK 21+; see docs/classloader.md"]
fn reads_validated_module_metadata_from_real_jdk_modules() {
    let jmods = jmods_directory()
        .canonicalize()
        .expect("the supplied JDK jmods directory must exist");
    let class_path = JdkClassPath::new(&jmods).expect("the supplied JMOD directory must open");
    let catalog = class_path
        .module_catalog()
        .expect("real JDK module descriptors must validate");

    let names = catalog
        .modules()
        .iter()
        .map(|module| module.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));

    let java_base = catalog.get("java.base").expect("java.base is in the JDK");
    assert_eq!(java_base.name, "java.base");
    assert_eq!(
        java_base.origin,
        ClassOrigin::Jmod(jmods.join("java.base.jmod"))
    );
    assert!(java_base.version.is_some());

    let http = catalog
        .get("java.net.http")
        .expect("java.net.http is in the JDK");
    assert_eq!(
        http.origin,
        ClassOrigin::Jmod(jmods.join("java.net.http.jmod"))
    );
    assert_eq!(
        http.requires
            .iter()
            .map(|row| row.module.as_str())
            .collect::<Vec<_>>(),
        ["java.base"]
    );
    assert!(
        http.exports
            .iter()
            .any(|row| row.package == "java.net.http" && row.targets.is_empty())
    );

    // Metadata inspection is explicit and does not affect regular lookup.
    assert_real_class(
        &class_path,
        &jmods,
        "java.net.http",
        "java/net/http/HttpClient",
    );
}

#[test]
#[ignore = "requires JDK 21 jmods plus the matching jmod CLI; see docs/classloader.md"]
fn checks_typed_jdk21_descriptors_against_jmod_describe() {
    let jmods = jmods_directory()
        .canonicalize()
        .expect("the supplied JDK jmods directory must exist");
    let java_home = std::env::var_os("JAVA_HOME").map(PathBuf::from);
    let jmod =
        jmod_executable(&jmods, java_home.as_deref()).unwrap_or_else(|error| panic!("{error}"));
    let class_path = JdkClassPath::new(&jmods).expect("the supplied JMOD directory must open");
    let catalog = class_path
        .module_catalog()
        .expect("real JDK module descriptors must validate");
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/jdk21-module-gate-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("gate scratch directory is writable");

    for name in ["java.base", "java.net.http", "jdk.jshell"] {
        let module = catalog
            .get(name)
            .unwrap_or_else(|| panic!("{name} is in JDK 21"));
        let jmod_path = jmods.join(format!("{name}.jmod"));
        assert_eq!(module.origin, ClassOrigin::Jmod(jmod_path.clone()));
        let extracted = scratch.join(name);
        std::fs::create_dir_all(&extracted).expect("module extraction directory is writable");
        run_jmod(
            &jmod,
            &[
                "extract",
                "--dir",
                extracted.to_str().unwrap(),
                jmod_path.to_str().unwrap(),
            ],
        );
        let bytes =
            std::fs::read(extracted.join("classes/module-info.class")).unwrap_or_else(|error| {
                panic!("{name} JMOD has no readable module-info.class: {error}")
            });
        let mut reader = Reader::new(&bytes);
        let descriptor = ClassFile::decode(&mut reader)
            .unwrap_or_else(|error| panic!("{name} module-info.class did not decode: {error}"));
        assert!(
            reader.is_at_end(),
            "{name} descriptor has no trailing bytes"
        );
        assert_eq!(
            descriptor.version.major, 65,
            "{name} is the Java 21 descriptor"
        );
        assert_raw_descriptor_matches_catalog(&descriptor, module);

        let description = run_jmod(&jmod, &["describe", jmod_path.to_str().unwrap()]);
        let actual = description
            .lines()
            .filter(|line| {
                let words = line.split_whitespace().collect::<Vec<_>>();
                matches!(
                    words.first().copied(),
                    Some("exports" | "opens" | "requires" | "uses" | "provides" | "main-class")
                ) || words.starts_with(&["qualified", "exports"])
                    || words.starts_with(&["qualified", "opens"])
                    || line.starts_with(&format!("{name}@"))
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let expected = jmod_observable_lines(module);
        let mut actual = actual;
        actual.sort();
        if actual != expected {
            let actual_set = actual.iter().collect::<std::collections::HashSet<_>>();
            let expected_set = expected.iter().collect::<std::collections::HashSet<_>>();
            let missing = expected_set
                .difference(&actual_set)
                .take(8)
                .collect::<Vec<_>>();
            let extra = actual_set
                .difference(&expected_set)
                .take(8)
                .collect::<Vec<_>>();
            panic!("jmod describe mismatch for {name}; missing={missing:?}, extra={extra:?}");
        }
        let mut described_packages = description
            .lines()
            .filter(|line| line.starts_with("contains "))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        described_packages.sort();
        let listed_packages = jmod_listed_packages(&jmod, &jmod_path, module);
        if described_packages != listed_packages {
            let described = described_packages
                .iter()
                .collect::<std::collections::HashSet<_>>();
            let listed = listed_packages
                .iter()
                .collect::<std::collections::HashSet<_>>();
            let missing = listed.difference(&described).take(8).collect::<Vec<_>>();
            let extra = described.difference(&listed).take(8).collect::<Vec<_>>();
            panic!(
                "jmod package inventory mismatch for {name}; missing={missing:?}, extra={extra:?}"
            );
        }
    }

    let base = catalog.get("java.base").unwrap();
    assert!(
        base.exports.iter().any(|row| !row.targets.is_empty()),
        "java.base covers qualified exports"
    );
    let http = catalog.get("java.net.http").unwrap();
    assert!(
        http.exports
            .iter()
            .any(|row| row.package == "java.net.http" && row.targets.is_empty())
    );
    let jshell = catalog.get("jdk.jshell").unwrap();
    assert!(
        !jshell.uses.is_empty() && !jshell.provides.is_empty(),
        "jdk.jshell covers service uses/provides"
    );
    assert!(
        [base, http, jshell]
            .iter()
            .all(|module| module.main_class.is_none()),
        "these JDK 21 descriptors omit ModuleMainClass"
    );

    let source = scratch.join("main-source");
    let classes = scratch.join("main-classes");
    let main_jmods = scratch.join("main-jmods");
    std::fs::create_dir_all(source.join("compat")).unwrap();
    std::fs::create_dir_all(&classes).unwrap();
    std::fs::create_dir_all(&main_jmods).unwrap();
    std::fs::write(
        source.join("module-info.java"),
        "module compat.main { exports compat; }\n",
    )
    .unwrap();
    std::fs::write(
        source.join("compat/Main.java"),
        "package compat; public final class Main {}\n",
    )
    .unwrap();
    let javac = jmod
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "javac.exe" } else { "javac" });
    run_tool(
        &javac,
        &[
            "-d",
            classes.to_str().unwrap(),
            source.join("module-info.java").to_str().unwrap(),
            source.join("compat/Main.java").to_str().unwrap(),
        ],
    );
    let main_jmod = main_jmods.join("compat.main.jmod");
    run_jmod(
        &jmod,
        &[
            "create",
            "--class-path",
            classes.to_str().unwrap(),
            "--main-class",
            "compat.Main",
            main_jmod.to_str().unwrap(),
        ],
    );
    let main_catalog = JdkClassPath::new(&main_jmods)
        .unwrap()
        .module_catalog()
        .unwrap();
    assert_eq!(
        main_catalog
            .get("compat.main")
            .unwrap()
            .main_class
            .as_deref(),
        Some("compat.Main")
    );
    let main_description = run_jmod(&jmod, &["describe", main_jmod.to_str().unwrap()]);
    assert!(
        main_description
            .lines()
            .any(|line| line == "main-class compat.Main"),
        "jmod describe prints ModuleMainClass as a main-class row"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
