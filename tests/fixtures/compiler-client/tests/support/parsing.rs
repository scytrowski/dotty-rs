use dotty::core::SourceId;
use dotty::{DetachedSource, parse_source};

pub fn parse_files(files: &[(SourceId, &str, &str)], workers: usize) -> Vec<DetachedSource> {
    let batch = files.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = files
            .chunks(batch)
            .map(|chunk| {
                scope.spawn(|| {
                    chunk
                        .iter()
                        .map(|&(id, _, text)| parse_source(id, text, Default::default()).unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        tasks
            .into_iter()
            .flat_map(|task| task.join().unwrap())
            .collect()
    })
}
