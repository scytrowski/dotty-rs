use dotty_classfile::access_flags::{ClassAccessFlags, MethodAccessFlags};
use dotty_classfile::attribute::{Attribute, CodeAttribute};
use dotty_classfile::class_file::{ClassFile, ClassFileVersion};
use dotty_classfile::constant_pool::{ConstantPool, ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::field::FieldInfo;
use dotty_classfile::method::MethodInfo;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dotty-classfile-jvm-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap_or_else(|error| {
            panic!("create temporary directory {}: {error}", path.display())
        });
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn executable_available(executable: &str) -> bool {
    Command::new(executable)
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn require_jdk_or_skip() -> bool {
    let missing: Vec<&str> = ["java", "javac", "javap"]
        .into_iter()
        .filter(|executable| !executable_available(executable))
        .collect();
    if missing.is_empty() {
        return true;
    }

    let message = format!(
        "JVM encoder gate requires JDK tools on PATH; unavailable: {}",
        missing.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}; CI sets DOTTY_CLASSFILE_REQUIRE_JDK=1 to make this gate mandatory");
    }
    eprintln!("skipping JVM encoder gate: {message}");
    false
}

fn run(executable: &str, args: &[&str]) -> Output {
    Command::new(executable)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to start {executable}: {error}"))
}

fn answer_class_file() -> ClassFile<'static> {
    let constant_pool = ConstantPool::from_entries(vec![
        Some(ConstantPoolEntry::Utf8("Answer".to_owned())),
        Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(1),
        }),
        Some(ConstantPoolEntry::Utf8("java/lang/Object".to_owned())),
        Some(ConstantPoolEntry::Class {
            name_index: ConstantPoolIndex(3),
        }),
        Some(ConstantPoolEntry::Utf8("answer".to_owned())),
        Some(ConstantPoolEntry::Utf8("()I".to_owned())),
        Some(ConstantPoolEntry::Utf8("Code".to_owned())),
    ]);
    let answer_method = MethodInfo {
        access_flags: MethodAccessFlags(0x0009), // public static
        name_index: ConstantPoolIndex(5),
        descriptor_index: ConstantPoolIndex(6),
        attributes: vec![Attribute::Code(CodeAttribute {
            max_stack: 1,
            max_locals: 0,
            code: &[0x10, 42, 0xAC], // bipush 42; ireturn
            exception_table: vec![],
            attributes: vec![],
        })],
    };

    ClassFile {
        version: ClassFileVersion {
            major: 65,
            minor: 0,
        }, // Java 21
        constant_pool,
        access_flags: ClassAccessFlags(0x0021), // public, super
        this_class: ConstantPoolIndex(2),
        super_class: Some(ConstantPoolIndex(4)),
        interfaces: vec![],
        fields: Vec::<FieldInfo<'static>>::new(),
        methods: vec![answer_method],
        attributes: vec![],
    }
}

#[test]
fn public_encoder_emits_a_class_accepted_and_executed_by_the_jvm() {
    if !require_jdk_or_skip() {
        return;
    }

    let directory = TempDir::new();
    let answer_path = directory.path().join("Answer.class");
    let class_file = answer_class_file();
    let bytes = class_file
        .encode()
        .expect("the public structural model is a valid class file");
    fs::write(&answer_path, bytes)
        .unwrap_or_else(|error| panic!("write {}: {error}", answer_path.display()));

    let harness_path = directory.path().join("AnswerHarness.java");
    fs::write(
        &harness_path,
        "public final class AnswerHarness {\n    public static void main(String[] args) {\n        int answer = Answer.answer();\n        if (answer != 42) throw new AssertionError(\"expected 42, got \" + answer);\n        System.out.println(answer);\n    }\n}\n",
    )
    .unwrap_or_else(|error| panic!("write {}: {error}", harness_path.display()));

    let directory_arg = directory.path().to_string_lossy().into_owned();
    let javap = run("javap", &["-v", "-classpath", &directory_arg, "Answer"]);
    assert!(
        javap.status.success(),
        "javap -v rejected generated Answer.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    assert!(
        javap_output.contains("public static int answer();"),
        "{javap_output}"
    );
    assert!(javap_output.contains("bipush        42"), "{javap_output}");
    assert!(javap_output.contains("ireturn"), "{javap_output}");

    let javac = run(
        "javac",
        &[
            "-classpath",
            &directory_arg,
            "-d",
            &directory_arg,
            harness_path.to_str().unwrap(),
        ],
    );
    assert!(
        javac.status.success(),
        "javac failed to compile the invocation harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );

    let java = run(
        "java",
        &[
            "-Xverify:all",
            "-classpath",
            &directory_arg,
            "AnswerHarness",
        ],
    );
    assert!(
        java.status.success(),
        "java -Xverify:all rejected or failed to invoke generated Answer.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "42");
}
