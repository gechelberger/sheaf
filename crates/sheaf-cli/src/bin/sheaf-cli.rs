//! `sheaf-cli`: the command line without the GUI, for servers and CI.

fn main() -> std::process::ExitCode {
    env_logger::init();
    std::process::ExitCode::from(sheaf_cli::run(std::env::args_os()))
}
