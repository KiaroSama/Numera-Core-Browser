#[cfg(target_os = "linux")]
mod prepare;

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    let result = prepare::run();
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), Box<dyn std::error::Error>> =
        Err("Capacity preparation is supported on the authorized Linux runner".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
