use pyo3::prelude::*;
use pyo3::wrap_pyfunction;

use wtg_cli::run_with_args;

/// Entry point used by the Python package to execute the CLI.
#[pyfunction]
fn run_cli(argv: Vec<String>) -> i32 {
    let cli_args = if argv.is_empty() {
        vec!["wtg".to_string()]
    } else {
        argv
    };

    match run_with_args(cli_args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            err.exit_code()
        }
    }
}

#[pymodule]
fn _wtg(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    Ok(())
}
