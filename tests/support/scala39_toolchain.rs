//! Shared pinned Scala 3.9.0/JDK preflight for strict TASTy consumers.

use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";

pub(crate) struct PinnedScala {
    pub(crate) java_home: PathBuf,
    pub(crate) scalac: PathBuf,
    pub(crate) java: PathBuf,
    pub(crate) libraries: String,
}

pub(crate) fn from_environment(
    mut output: impl FnMut(&mut Command) -> std::process::Output,
    configure_version: impl FnOnce(&mut Command),
) -> PinnedScala {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required"));
    let maven = PathBuf::from(
        std::env::var_os("COURSIER_MAVEN_ROOT").expect("COURSIER_MAVEN_ROOT is required"),
    );
    let libraries = [
        (
            maven.join("org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar"),
            "9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba",
        ),
        (
            maven.join("org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar"),
            "b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd",
        ),
    ];
    for (path, expected) in &libraries {
        assert!(
            path.is_file(),
            "pinned published Scala runtime exists: {}",
            path.display()
        );
        assert_eq!(
            sha256(path, &mut output),
            *expected,
            "published Scala runtime hash {}",
            path.display()
        );
    }

    let revision = output(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "HEAD"]),
    );
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        SCALA_REVISION
    );

    let stage = [
        root.join("dist/target/universal/stage"),
        root.join("dist/linux-x86_64/target/universal/stage"),
    ]
    .into_iter()
    .find(|stage| {
        stage.join("bin/scalac").is_file() && stage.join("lib/with_compiler.jar").is_file()
    })
    .expect("pinned staged Scala compiler and with_compiler.jar exist");
    let scalac = stage.join("bin/scalac");
    let mut version = Command::new(&scalac);
    version.arg("-version");
    configure_version(&mut version);
    let version = output(&mut version);
    let version_text = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(
        version_text.contains("3.9.0-bin-SNAPSHOT-git-777528f"),
        "{version_text}"
    );

    let compiler_jar = stage.join("lib/with_compiler.jar");
    assert!(
        compiler_jar.is_file(),
        "staged pinned compiler jar exists: {}",
        compiler_jar.display()
    );
    let java = java_home.join("bin/java");
    let java_version = output(Command::new(&java).arg("-version"));
    let java_text = format!(
        "{}{}",
        String::from_utf8_lossy(&java_version.stdout),
        String::from_utf8_lossy(&java_version.stderr)
    );
    assert!(java_text.contains("21.0.13"), "{java_text}");
    assert_eq!(
        sha256(&java, &mut output),
        "9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155"
    );
    eprintln!(
        "SCALA_PROVENANCE revision={SCALA_REVISION} compiler={} compiler_sha256={} java={} java_sha256={}",
        version_text.trim(),
        sha256(&compiler_jar, &mut output),
        java.display(),
        sha256(&java, &mut output)
    );

    PinnedScala {
        java_home,
        scalac,
        java,
        libraries: libraries
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect::<Vec<_>>()
            .join(":"),
    }
}

pub(crate) fn scalac(java_home: &Path, scalac: &Path, temp_root: PathBuf) -> Command {
    let mut command = Command::new(scalac);
    command.env("JAVA_HOME", java_home);
    command.env(
        "JAVA_TOOL_OPTIONS",
        format!("-XX:-UsePerfData -Djava.io.tmpdir={}", temp_root.display()),
    );
    if let Some(path) = std::env::var_os("PATH") {
        let mut paths = vec![java_home.join("bin")];
        paths.extend(std::env::split_paths(&path));
        command.env("PATH", std::env::join_paths(paths).expect("PATH is valid"));
    }
    command
}

pub(crate) fn sha256(
    path: &Path,
    output: &mut impl FnMut(&mut Command) -> std::process::Output,
) -> String {
    String::from_utf8_lossy(&output(Command::new("sha256sum").arg(path)).stdout)
        .split_whitespace()
        .next()
        .expect("sha256sum emits a hash")
        .to_owned()
}
