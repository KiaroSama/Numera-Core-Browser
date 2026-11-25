#[path = "../maintenance/licenses.rs"]
mod licenses;

fn main() -> std::process::ExitCode {
    licenses::exit(false)
}
