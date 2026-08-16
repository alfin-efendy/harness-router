use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    // Nothing in this workspace installs a tracing subscriber by default, so
    // every `tracing::…!` event in ryuzi-core is discarded until this runs.
    // Level is controlled by RYUZI_LOG (default `info`). Output goes to
    // stderr, which the daemon spawn path redirects into daemon.log.
    ryuzi_core::logging::init_tracing();

    let mut deps = ryuzi_control::dispatch::Deps {
        db_path: ryuzi_core::paths::db_path(),
        out: Box::new(|s| println!("{s}")),
        err: Box::new(|s| eprintln!("{s}")),
        prompt: Box::new(|q| {
            print!("{q}");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            line
        }),
        detect_git: ryuzi_control::detect::detect_git,
    };
    ExitCode::from(ryuzi_control::dispatch::run_cli(
        std::env::args().skip(1).collect(),
        &mut deps,
    ))
}
