//! Benchmark-only observations; no changes to classpath policy or caching.
use dotty_classloader::classloader::{ClassOrigin, JdkClassPath, JdkModuleCatalog};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::time::Instant;

const LOOKUPS: [&str; 4] = [
    "java.base",
    "java.net.http",
    "java.desktop",
    "dotty.missing.module",
];
const CACHED_ITERATIONS: usize = 10_000;
const FRESH_ITERATIONS: usize = 20;
const THREADS: usize = 4;
const THREAD_ITERATIONS: usize = 10;

pub(super) fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    if args.len() != 4 || args[0] != "--jmods" || args[2] != "--sessions" {
        return Err(
            "usage: jdk_classpath_benchmark --metadata --jmods PATH --sessions 1|2|4".into(),
        );
    }
    let jmods = PathBuf::from(&args[1])
        .canonicalize()
        .map_err(|error| format!("JMOD directory is unavailable: {error}"))?;
    let sessions = args[3]
        .to_str()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| matches!(count, 1 | 2 | 4))
        .ok_or("--sessions must be 1, 2 or 4")?;
    measure(&jmods, sessions)
}

fn read_catalog(class_path: &JdkClassPath) -> Result<JdkModuleCatalog, String> {
    class_path
        .module_catalog()
        .map_err(|error| format!("module catalog failed: {error}"))
}

fn validate(catalog: &JdkModuleCatalog, jmods: &Path) -> Result<(), String> {
    let modules = catalog.modules();
    let expected_count = std::fs::read_dir(jmods)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .iter()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jmod"))
        .count();
    if modules.len() != expected_count
        || !modules.windows(2).all(|pair| pair[0].name < pair[1].name)
    {
        return Err("catalog count/order differs from JMOD directory".into());
    }
    for module in modules {
        if module.origin != ClassOrigin::Jmod(jmods.join(format!("{}.jmod", module.name))) {
            return Err(format!("wrong source identity for {}", module.name));
        }
    }
    let base = catalog.get("java.base").ok_or("missing java.base")?;
    if base.version.is_none() || !base.requires.is_empty() {
        return Err("unexpected java.base metadata".into());
    }
    let http = catalog
        .get("java.net.http")
        .ok_or("missing java.net.http")?;
    if http
        .requires
        .iter()
        .map(|row| row.module.as_str())
        .collect::<Vec<_>>()
        != ["java.base"]
        || !http
            .exports
            .iter()
            .any(|row| row.package == "java.net.http" && row.targets.is_empty())
    {
        return Err("unexpected java.net.http directives".into());
    }
    let desktop = catalog.get("java.desktop").ok_or("missing java.desktop")?;
    if desktop.uses.is_empty() || desktop.provides.is_empty() {
        return Err("java.desktop must exercise both uses and provides".into());
    }
    if catalog.get(LOOKUPS[3]).is_some() {
        return Err("missing-module lookup unexpectedly succeeded".into());
    }
    Ok(())
}

/// Called in a separate process by the runner before collecting timed samples.
pub(super) fn check(jmods: &Path) -> Result<(), String> {
    let jmods = jmods.canonicalize().map_err(|error| error.to_string())?;
    let class_path = JdkClassPath::new(&jmods).map_err(|error| error.to_string())?;
    validate(&read_catalog(&class_path)?, &jmods)?;
    println!("{{\"metadata_check\":true}}");
    Ok(())
}

/// Fixture mode used by the Python runner without javac/java or a local JDK.
pub(super) fn smoke(jmods: &Path) -> Result<(), String> {
    let jmods = jmods.canonicalize().map_err(|error| error.to_string())?;
    let path = JdkClassPath::new(&jmods).map_err(|error| error.to_string())?;
    let catalog = read_catalog(&path)?;
    if catalog
        .modules()
        .iter()
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>()
        != ["fixture.alpha", "fixture.beta", "fixture.minimal"]
    {
        return Err("unexpected smoke module names/order".into());
    }
    let alpha = catalog
        .get("fixture.alpha")
        .ok_or("missing fixture.alpha")?;
    if alpha.version.as_deref() != Some("1.2")
        || alpha.origin != ClassOrigin::Jmod(jmods.join("fixture.alpha.jmod"))
        || alpha.uses != ["fixture.alpha.api.Service"]
        || alpha.provides.len() != 1
        || alpha.provides[0].providers != ["fixture.alpha.impl.Provider"]
        || catalog.get(LOOKUPS[3]).is_some()
    {
        return Err("unexpected smoke metadata projection".into());
    }
    if catalog != read_catalog(&path)? {
        return Err("repeated smoke catalog differs".into());
    }
    println!("{{\"metadata_smoke\":true,\"module_count\":3}}");
    Ok(())
}

fn check_lookup(catalog: &JdkModuleCatalog, name: &str) -> Result<(), String> {
    let found = black_box(catalog).get(black_box(name));
    if (name == LOOKUPS[3]) != found.is_none() {
        return Err(format!("unexpected module lookup result: {name}"));
    }
    black_box(found);
    Ok(())
}

fn enumerate(catalog: &JdkModuleCatalog) -> usize {
    black_box(catalog)
        .modules()
        .iter()
        .map(|module| black_box(module.name.len()))
        .sum()
}

fn measure(jmods: &Path, sessions: usize) -> Result<(), String> {
    let before_init = resources()?;
    let mut paths = Vec::with_capacity(sessions);
    let mut init_times = Vec::new();
    let start = Instant::now();
    for _ in 0..sessions {
        let start = Instant::now();
        paths.push(JdkClassPath::new(jmods).map_err(|error| error.to_string())?);
        init_times.push(start.elapsed().as_nanos());
    }
    let init_ns = start.elapsed().as_nanos();
    let after_init = resources()?;
    // No catalog call (even validation) has occurred before this checkpoint.
    let mut catalogs = Vec::with_capacity(sessions);
    let mut first_catalog_ns = Vec::new();
    for path in &paths {
        let start = Instant::now();
        let catalog = read_catalog(path)?;
        first_catalog_ns.push(start.elapsed().as_nanos());
        // Validate outside the catalog timer; the runner also preflights separately.
        validate(&catalog, jmods)?;
        catalogs.push(catalog);
    }
    let after_first_catalogs = resources()?;
    let mut rows = Vec::new();
    for name in LOOKUPS {
        let start = Instant::now();
        check_lookup(&catalogs[0], name)?;
        let first_snapshot_lookup_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        for _ in 0..CACHED_ITERATIONS {
            check_lookup(&catalogs[0], name)?;
        }
        let snapshot_total_ns = start.elapsed().as_nanos();
        let mut fresh_total_ns = 0;
        for _ in 0..FRESH_ITERATIONS {
            let start = Instant::now();
            let catalog = read_catalog(&paths[0])?;
            check_lookup(&catalog, name)?;
            fresh_total_ns += start.elapsed().as_nanos();
            // Temporary snapshot destruction is outside the timed read+get operation.
        }
        rows.push(format!(
            "{{\"module\":\"{name}\",\"first_snapshot_lookup_ns\":{first_snapshot_lookup_ns},\"snapshot_iterations\":{CACHED_ITERATIONS},\"snapshot_total_ns\":{snapshot_total_ns},\"fresh_iterations\":{FRESH_ITERATIONS},\"fresh_total_ns\":{fresh_total_ns}}}"
        ));
    }
    let start = Instant::now();
    for _ in 0..CACHED_ITERATIONS {
        black_box(enumerate(&catalogs[0]));
    }
    let enumeration_total_ns = start.elapsed().as_nanos();
    let shared = concurrent(&paths, false)?;
    let independent = concurrent(&paths, true)?;
    let after_calls = resources()?;
    let module_count = catalogs[0].modules().len();
    let string_bytes = string_bytes(&catalogs[0]);
    drop(catalogs);
    let after_drop_catalogs = resources()?;
    drop(paths);
    let after_drop_paths = resources()?;
    println!(
        "{{\"sessions\":{sessions},\"module_count\":{module_count},\"catalog_string_bytes_per_session\":{string_bytes},\"init_ns\":{init_ns},\"init_per_session_ns\":{init_times:?},\"first_catalog_per_session_ns\":{first_catalog_ns:?},\"before_init\":{before_init},\"after_init\":{after_init},\"after_first_catalogs\":{after_first_catalogs},\"after_calls\":{after_calls},\"after_drop_catalogs\":{after_drop_catalogs},\"after_drop_paths\":{after_drop_paths},\"module_lookups\":[{}],\"enumeration\":{{\"iterations\":{CACHED_ITERATIONS},\"total_ns\":{enumeration_total_ns}}},\"concurrent\":[{shared},{independent}]}}",
        rows.join(",")
    );
    Ok(())
}

fn concurrent(paths: &[JdkClassPath], independent: bool) -> Result<String, String> {
    let ready = Barrier::new(THREADS + 1);
    let gate = Barrier::new(THREADS + 1);
    let total_ns = std::thread::scope(|scope| -> Result<u128, String> {
        let readers: Vec<_> = (0..THREADS)
            .map(|index| {
                let ready = &ready;
                let gate = &gate;
                let path = &paths[if independent { index % paths.len() } else { 0 }];
                scope.spawn(move || -> Result<(), String> {
                    ready.wait();
                    gate.wait();
                    for _ in 0..THREAD_ITERATIONS {
                        let catalog = read_catalog(path)?;
                        check_lookup(&catalog, LOOKUPS[index])?;
                    }
                    Ok(())
                })
            })
            .collect();
        let start = super::start_timed_workers(&ready, &gate, || {});
        for reader in readers {
            reader.join().map_err(|_| "metadata worker panicked")??;
        }
        Ok(start.elapsed().as_nanos())
    })?;
    let mode = if independent {
        "independent_sessions"
    } else {
        "shared_session"
    };
    Ok(format!(
        "{{\"mode\":\"{mode}\",\"threads\":{THREADS},\"iterations_per_thread\":{THREAD_ITERATIONS},\"total_ns\":{total_ns}}}"
    ))
}

fn resources() -> Result<String, String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|error| error.to_string())?;
    let parse = |key: &str| -> Result<u64, String> {
        status
            .lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("missing {key} in /proc/self/status"))
    };
    let rss = parse("VmRSS:")?;
    let hwm = parse("VmHWM:")?;
    let fds = super::open_fd_count()?;
    Ok(format!(
        "{{\"rss_kib\":{rss},\"peak_rss_kib\":{hwm},\"fd_count\":{fds}}}"
    ))
}

// Logical UTF-8 payload only, not an allocator/heap-size estimate.
fn string_bytes(catalog: &JdkModuleCatalog) -> usize {
    catalog
        .modules()
        .iter()
        .map(|module| {
            module.name.len()
                + module.version.as_ref().map_or(0, String::len)
                + module
                    .requires
                    .iter()
                    .map(|row| row.module.len() + row.version.as_ref().map_or(0, String::len))
                    .sum::<usize>()
                + module
                    .exports
                    .iter()
                    .map(|row| {
                        row.package.len() + row.targets.iter().map(String::len).sum::<usize>()
                    })
                    .sum::<usize>()
                + module
                    .opens
                    .iter()
                    .map(|row| {
                        row.package.len() + row.targets.iter().map(String::len).sum::<usize>()
                    })
                    .sum::<usize>()
                + module.uses.iter().map(String::len).sum::<usize>()
                + module
                    .provides
                    .iter()
                    .map(|row| {
                        row.service.len() + row.providers.iter().map(String::len).sum::<usize>()
                    })
                    .sum::<usize>()
                + module
                    .packages
                    .as_ref()
                    .map_or(0, |rows| rows.iter().map(String::len).sum::<usize>())
                + module.main_class.as_ref().map_or(0, String::len)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_classloader::classloader::JdkModuleCatalogError;

    #[test]
    fn missing_descriptors_fail_only_on_explicit_metadata_access() {
        // A valid empty JMOD proves construction needs no module-info payload.
        let fixture = std::env::temp_dir().join(format!(
            "dotty-metadata-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&fixture).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(fixture.clone());
        let mut archive = b"JM\x01\x00PK\x05\x06".to_vec();
        archive.extend([0; 18]);
        std::fs::write(fixture.join("empty.jmod"), archive).unwrap();
        let paths: Vec<_> = (0..4)
            .map(|_| JdkClassPath::new(&fixture).unwrap())
            .collect();
        for path in paths {
            assert!(matches!(
                path.module_catalog(),
                Err(JdkModuleCatalogError::MissingDescriptor { .. })
            ));
        }
    }

    #[test]
    fn snapshot_lookup_checks_both_positive_and_missing_results() {
        let empty = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let catalog = JdkClassPath::new(empty).unwrap().module_catalog().unwrap();
        assert!(check_lookup(&catalog, "java.base").is_err());
        assert!(check_lookup(&catalog, LOOKUPS[3]).is_ok());
        assert_eq!(enumerate(&catalog), 0);
        assert_eq!(string_bytes(&catalog), 0);
    }
}
