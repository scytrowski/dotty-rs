use dotty::core::{SourceId, SourceText};
use dotty::lexer::ContextualScanner;
use dotty::parser::DetachedParse;

pub fn parse_files(files: &[(SourceId, &str, &str)], workers: usize) -> Vec<DetachedParse> {
    let batch = files.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = files
            .chunks(batch)
            .map(|chunk| {
                scope.spawn(|| {
                    chunk
                        .iter()
                        .map(|&(id, _, text)| {
                            let scanner = ContextualScanner::new(text).unwrap();
                            assert!(
                                scanner.diagnostics().is_empty(),
                                "{:?}",
                                scanner.diagnostics()
                            );
                            DetachedParse::parse(
                                SourceText::new(text).unwrap(),
                                id,
                                scanner,
                                Default::default(),
                            )
                        })
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
