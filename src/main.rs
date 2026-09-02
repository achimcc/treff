use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version") {
        println!("treff {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    eprintln!("treff: no configuration; set TREFF_CONFIG and the TREFF_OIDC_* variables");
    ExitCode::FAILURE
}
