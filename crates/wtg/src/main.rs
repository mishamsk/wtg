fn main() {
    if let Err(err) = wtg::run() {
        eprintln!("{err}");
        std::process::exit(err.exit_code());
    }
}
