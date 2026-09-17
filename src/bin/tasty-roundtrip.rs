use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use dotty::tasty::TastyFile;

const COMPILER_MAJOR: u32 = 28;
const COMPILER_MINOR: u32 = 9;
const COMPILER_EXPERIMENTAL: u32 = 0;

fn main() {
    if let Err(error) = run() {
        eprintln!("tasty-roundtrip: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let mut mode = None;
    let mut positional = Vec::new();

    for argument in arguments.by_ref() {
        if let Some(value) = argument.strip_prefix("--mode=") {
            if mode.replace(value.to_owned()).is_some() {
                return Err(usage("round-trip mode specified more than once"));
            }
        } else if argument.starts_with('-') {
            return Err(usage("unknown option"));
        } else {
            positional.push(PathBuf::from(argument));
        }
    }

    if positional.len() != 2 {
        return Err(usage(if positional.is_empty() {
            "missing input and output directories"
        } else if positional.len() == 1 {
            "missing output directory"
        } else {
            "too many positional arguments"
        }));
    }
    let input = positional.remove(0);
    let output = positional.remove(0);
    let mode = mode.unwrap_or_else(|| "structured".to_owned());
    if mode != "structured" {
        return Err(usage("only structured mode is currently supported"));
    }
    if !input.is_dir() {
        return Err(format!(
            "input directory does not exist: {}",
            input.display()
        ));
    }
    if output.exists() && !output.is_dir() {
        return Err(format!(
            "output path is not a directory: {}",
            output.display()
        ));
    }

    let mut paths = Vec::new();
    collect_tasty_files(&input, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        return Err(format!(
            "input directory contains no .tasty files: {}",
            input.display()
        ));
    }
    fs::create_dir_all(&output)
        .map_err(|error| format!("failed to create {}: {error}", output.display()))?;

    for path in &paths {
        let relative = path
            .strip_prefix(&input)
            .map_err(|error| format!("{} is outside input directory: {error}", path.display()))?;
        let output_path = output.join(relative);
        let parent = output_path.parent().ok_or_else(|| {
            format!(
                "output path has no parent directory: {}",
                output_path.display()
            )
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;

        let bytes = fs::read(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let file = TastyFile::parse_compatible_with(
            &bytes,
            COMPILER_MAJOR,
            COMPILER_MINOR,
            COMPILER_EXPERIMENTAL,
        )
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        let nodes = file.structured_asts().map_err(|error| {
            format!("failed to decode {} structurally: {error}", path.display())
        })?;
        let encoded = file.encode_structured_relocated(&nodes).map_err(|error| {
            format!("failed to encode {} structurally: {error}", path.display())
        })?;
        let reparsed = TastyFile::parse_compatible_with(
            &encoded,
            COMPILER_MAJOR,
            COMPILER_MINOR,
            COMPILER_EXPERIMENTAL,
        )
        .map_err(|error| format!("failed to reparse {}: {error}", relative.display()))?;
        reparsed
            .validate()
            .map_err(|error| format!("failed to validate {}: {error}", relative.display()))?;
        fs::write(&output_path, encoded)
            .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
    }

    println!(
        "re-encoded {} TASTy files from {} to {}",
        paths.len(),
        input.display(),
        output.display()
    );
    Ok(())
}

fn usage(message: &str) -> String {
    format!(
        "{message}\nusage: tasty-roundtrip [--mode=structured] <input-directory> <output-directory>"
    )
}

fn collect_tasty_files(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to inspect an entry in {}: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_tasty_files(&path, paths)?;
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "tasty")
        {
            paths.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::usage;

    #[test]
    fn documents_the_structured_round_trip_command() {
        assert!(usage("bad arguments").contains("<input-directory> <output-directory>"));
    }
}
