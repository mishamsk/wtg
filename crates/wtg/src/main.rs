fn main() {
    if let Err(err) = wtg_cli::run() {
        eprintln!("{err}");
        std::process::exit(err.exit_code());
    }
}
