#[cfg(windows)]
#[allow(unsafe_code)]
mod trust;

#[cfg(windows)]
mod sdk;

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    let result = sdk::run();
    #[cfg(not(windows))]
    let result: Result<(), Box<dyn std::error::Error>> =
        Err("SDK preparation requires the authorized Windows GitHub-hosted runner".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Windows SDK preparation failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
