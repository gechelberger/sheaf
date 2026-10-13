//! Placeholder `sheaf` binary: prints where to get the real app.

const MESSAGE: &str = "\
This `sheaf` is the crates.io placeholder (sheaf-pdf), not the app.

Sheaf can't be built from crates.io. Instead:

  Prebuilt bundles:   https://github.com/gechelberger/sheaf/releases
  Build from source:  https://github.com/gechelberger/sheaf#building

To remove this placeholder:  cargo uninstall sheaf-pdf
";

fn main() {
    eprint!("{MESSAGE}");
    std::process::exit(1);
}
