use dotty_classloader::classloader::{BinaryName, ClassOrigin, ClassPathEntry, JdkClassPath};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

const WARM_ITERATIONS: u64 = 1_000;
const PACKAGE_ITERATIONS: u64 = 1_000;
const CONCURRENT_THREADS: usize = 4;
const CONCURRENT_ITERATIONS_PER_THREAD: u64 = 250;

fn main() {
    if let Err(error) = run() {
        eprintln!("jdk-classpath-benchmark: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--smoke")) && args.next().is_none() {
        return smoke();
    }
    let jmods = match (first, args.next()) {
        (Some(flag), Some(path)) if flag == "--jmods" && args.next().is_none() => {
            PathBuf::from(path)
        }
        _ => return Err("usage: jdk_classpath_benchmark --jmods PATH".into()),
    };
    if !jmods.is_dir() {
        return Err(format!(
            "JMOD directory is unavailable: {}; pass --jmods PATH or use the tools runner with JAVA_HOME",
            jmods.display()
        ));
    }

    let init_start = Instant::now();
    let class_path = JdkClassPath::new(&jmods)
        .map_err(|error| format!("could not open JDK jmods at {}: {error}", jmods.display()))?;
    let init_ns = init_start.elapsed().as_nanos();
    let fd_count_after_init = open_fd_count()?;

    let mut samples = Vec::new();
    for (module, name) in [
        ("java.base", "java/lang/Object"),
        ("java.base", "java/lang/String"),
        ("java.base", "java/util/ArrayList"),
        ("java.base", "java/util/concurrent/CompletableFuture"),
        ("java.net.http", "java/net/http/HttpClient"),
    ] {
        let lookup_start = Instant::now();
        let resource = class_path
            .find_class(&BinaryName::from_internal(name))
            .map_err(|error| format!("lookup for {name} failed: {error}"))?
            .ok_or_else(|| format!("required JDK class was not found: {name}"))?;
        let first_ns = lookup_start.elapsed().as_nanos();
        let origin_module = match resource.origin() {
            ClassOrigin::Jmod(path) => path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_owned(),
            _ => "unexpected-origin".to_owned(),
        };
        if origin_module != module {
            return Err(format!(
                "{name} resolved from {origin_module}, expected {module}"
            ));
        }
        drop(resource);

        let warm_start = Instant::now();
        for _ in 0..WARM_ITERATIONS {
            class_path
                .find_class(&BinaryName::from_internal(name))
                .map_err(|error| format!("warm lookup for {name} failed: {error}"))?
                .ok_or_else(|| format!("warm lookup could not find required class: {name}"))?;
        }
        let warm_total_ns = warm_start.elapsed().as_nanos();
        samples.push(format!(
            "{{\"module\":\"{module}\",\"class\":\"{name}\",\"first_lookup_ns\":{first_ns},\"warm_iterations\":{WARM_ITERATIONS},\"warm_total_ns\":{warm_total_ns}}}"
        ));
    }

    let negative_name = "dotty_issue_1157/DefinitelyMissing";
    let negative_start = Instant::now();
    if class_path
        .find_class(&BinaryName::from_internal(negative_name))
        .map_err(|error| format!("negative lookup failed: {error}"))?
        .is_some()
    {
        return Err(format!(
            "missing-class assertion failed: {negative_name} was found"
        ));
    }
    let negative_first_ns = negative_start.elapsed().as_nanos();
    let negative_warm_start = Instant::now();
    for _ in 0..WARM_ITERATIONS {
        if class_path
            .find_class(&BinaryName::from_internal(negative_name))
            .map_err(|error| format!("warm negative lookup failed: {error}"))?
            .is_some()
        {
            return Err(format!(
                "missing-class assertion failed: {negative_name} was found"
            ));
        }
    }
    let negative_warm_total_ns = negative_warm_start.elapsed().as_nanos();

    let mut package_samples = Vec::new();
    for (query, package) in [
        ("contains_package", vec!["java", "lang"]),
        ("contains_package_prefix", vec!["java", "net", "http"]),
    ] {
        let start = Instant::now();
        for _ in 0..PACKAGE_ITERATIONS {
            let found = match query {
                "contains_package" => class_path.contains_package(&package),
                "contains_package_prefix" => class_path.contains_package_prefix(&package),
                _ => unreachable!(),
            }
            .map_err(|error| format!("package query {query} failed: {error}"))?;
            if !found {
                return Err(format!("package assertion failed: {query} {package:?}"));
            }
        }
        package_samples.push(format!(
            "{{\"query\":\"{query}\",\"package\":\"{}\",\"iterations\":{PACKAGE_ITERATIONS},\"total_ns\":{}}}",
            package.join("/"),
            start.elapsed().as_nanos()
        ));
    }
    let start = Instant::now();
    for _ in 0..PACKAGE_ITERATIONS {
        if !class_path
            .contains_package_prefix(&["java", "util", "concurrent"])
            .map_err(|error| format!("package-prefix query failed: {error}"))?
        {
            return Err("package-prefix assertion failed: java/util/concurrent".into());
        }
    }
    package_samples.push(format!(
        "{{\"query\":\"contains_package_prefix\",\"package\":\"java/util/concurrent\",\"iterations\":{PACKAGE_ITERATIONS},\"total_ns\":{}}}",
        start.elapsed().as_nanos()
    ));

    let concurrent_scenarios = [
        ("java.base", "java/lang/Object"),
        ("java.net.http", "java/net/http/HttpClient"),
        ("missing", "dotty_issue_1157/DefinitelyMissing"),
    ];
    let mut concurrent_samples = Vec::new();
    for (expected_module, name) in concurrent_scenarios {
        let class_path = Arc::new(&class_path);
        let ready = Arc::new(Barrier::new(CONCURRENT_THREADS + 1));
        let start_gate = Arc::new(Barrier::new(CONCURRENT_THREADS + 1));
        let elapsed = thread::scope(|scope| -> Result<u128, String> {
            let readers = (0..CONCURRENT_THREADS)
                .map(|_| {
                    let class_path = Arc::clone(&class_path);
                    let ready = Arc::clone(&ready);
                    let start_gate = Arc::clone(&start_gate);
                    scope.spawn(move || -> Result<(), String> {
                        ready.wait();
                        start_gate.wait();
                        for _ in 0..CONCURRENT_ITERATIONS_PER_THREAD {
                            let resource = class_path
                                .find_class(&BinaryName::from_internal(name))
                                .map_err(|error| format!("concurrent lookup for {name} failed: {error}"))?;
                            if expected_module == "missing" {
                                if resource.is_some() {
                                    return Err(format!("missing-class assertion failed: {name} was found"));
                                }
                            } else {
                                let resource = resource
                                    .ok_or_else(|| format!("concurrent lookup could not find {name}"))?;
                                let origin_module = match resource.origin() {
                                    ClassOrigin::Jmod(path) => path
                                        .file_stem()
                                        .and_then(|stem| stem.to_str())
                                        .unwrap_or("unknown"),
                                    _ => "unexpected-origin",
                                };
                                if origin_module != expected_module {
                                    return Err(format!(
                                        "{name} resolved from {origin_module}, expected {expected_module}"
                                    ));
                                }
                            }
                        }
                        Ok(())
                    })
                })
                .collect::<Vec<_>>();
            let start = start_timed_workers(&ready, &start_gate, || {});
            for reader in readers {
                reader
                    .join()
                    .map_err(|_| format!("concurrent lookup worker panicked for {name}"))??;
            }
            Ok(start.elapsed().as_nanos())
        })?;
        concurrent_samples.push(format!(
            "{{\"module\":\"{expected_module}\",\"class\":\"{name}\",\"threads\":{CONCURRENT_THREADS},\"iterations_per_thread\":{CONCURRENT_ITERATIONS_PER_THREAD},\"total_ns\":{elapsed}}}"
        ));
    }

    let fd_count_after_workload = open_fd_count()?;
    let peak_rss_kib = peak_rss_kib()?;
    println!(
        "{{\"jmods\":\"provided\",\"peak_rss_kib\":{peak_rss_kib},\"fd_count_after_init\":{fd_count_after_init},\"fd_count_after_workload\":{fd_count_after_workload},\"init_ns\":{init_ns},\"class_lookups\":[{}],\"negative_lookup\":{{\"class\":\"{negative_name}\",\"first_lookup_ns\":{negative_first_ns},\"warm_iterations\":{WARM_ITERATIONS},\"warm_total_ns\":{negative_warm_total_ns}}},\"package_lookups\":[{}],\"concurrent_lookups\":[{}]}}",
        samples.join(","),
        package_samples.join(","),
        concurrent_samples.join(",")
    );
    Ok(())
}

fn start_timed_workers(
    ready: &Barrier,
    start_gate: &Barrier,
    before_release: impl FnOnce(),
) -> Instant {
    ready.wait();
    let start = Instant::now();
    before_release();
    start_gate.wait();
    start
}

#[cfg(test)]
mod tests {
    use super::start_timed_workers;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn coordinator_delay_before_releasing_workers_is_included_in_timing() {
        let ready = Arc::new(Barrier::new(2));
        let start_gate = Arc::new(Barrier::new(2));
        let did_work = Arc::new(AtomicBool::new(false));

        thread::scope(|scope| {
            let worker_ready = Arc::clone(&ready);
            let worker_start_gate = Arc::clone(&start_gate);
            let worker_did_work = Arc::clone(&did_work);
            let worker = scope.spawn(move || {
                worker_ready.wait();
                worker_start_gate.wait();
                worker_did_work.store(true, Ordering::SeqCst);
            });

            let start = start_timed_workers(&ready, &start_gate, || {
                thread::sleep(Duration::from_millis(20));
                assert!(!did_work.load(Ordering::SeqCst));
            });
            worker.join().expect("worker should complete");

            assert!(did_work.load(Ordering::SeqCst));
            assert!(start.elapsed() >= Duration::from_millis(20));
        });
    }
}

fn peak_rss_kib() -> Result<u64, String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|error| {
        format!("Linux /proc/self/status is unavailable for RSS measurement: {error}")
    })?;
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .ok_or_else(|| "Linux /proc/self/status did not report VmHWM".to_owned())?;
    line.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("could not parse peak RSS from {line:?}"))
}

fn open_fd_count() -> Result<usize, String> {
    std::fs::read_dir("/proc/self/fd")
        .map_err(|error| {
            format!("Linux /proc/self/fd is unavailable for descriptor measurement: {error}")
        })?
        .try_fold(0, |count, entry| {
            entry
                .map(|_| count + 1)
                .map_err(|error| format!("could not count open file descriptors: {error}"))
        })
}

fn smoke() -> Result<(), String> {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jdk_classpath");
    let class_path = JdkClassPath::new(&fixture)
        .map_err(|error| format!("smoke fixture could not be opened: {error}"))?;
    for name in ["pool/PoolSample", "other/OtherSample"] {
        class_path
            .find_class(&BinaryName::from_internal(name))
            .map_err(|error| format!("smoke lookup for {name} failed: {error}"))?
            .ok_or_else(|| format!("smoke fixture is missing {name}"))?;
    }
    if class_path
        .find_class(&BinaryName::from_internal("DefinitelyMissing"))
        .map_err(|error| format!("smoke negative lookup failed: {error}"))?
        .is_some()
    {
        return Err("smoke negative lookup unexpectedly found a class".into());
    }
    if !class_path
        .contains_package(&["pool"])
        .map_err(|error| format!("smoke direct package query failed: {error}"))?
        || !class_path
            .contains_package_prefix(&["other"])
            .map_err(|error| format!("smoke package-prefix query failed: {error}"))?
    {
        return Err("smoke package query assertion failed".into());
    }
    let peak_rss_kib = peak_rss_kib()?;
    println!(
        "{{\"smoke\":true,\"peak_rss_kib\":{peak_rss_kib},\"fd_count_after_init\":{},\"positive_classes\":2,\"negative_lookup\":true,\"package_queries\":true}}",
        open_fd_count()?
    );
    Ok(())
}
