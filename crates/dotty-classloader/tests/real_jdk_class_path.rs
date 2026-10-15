//! Opt-in integration check against an installed JDK runtime image.
//!
//! Run with `DOTTY_JDK_JMODS=/path/to/jdk/jmods cargo test -p
//! dotty-classloader --test real_jdk_class_path -- --ignored`. If
//! `DOTTY_JDK_JMODS` is unset, the test requires `JAVA_HOME` and reads its
//! `jmods` directory. It is ignored in normal CI and never downloads a JDK.

use std::path::{Path, PathBuf};

use dotty_classfile::class_file::ClassFile;
use dotty_classfile::reader::Reader;
use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassOrigin, ClassPathEntry, JdkClassPath,
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
