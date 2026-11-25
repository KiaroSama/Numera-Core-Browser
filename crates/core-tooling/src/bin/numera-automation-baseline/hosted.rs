use super::{Result, failure};
pub use core_tooling::chromium_build::process::environment_value as value;
use core_tooling::fixtures::Environment;
use std::path::{Path, PathBuf};

pub fn driver(environment: &Environment) -> Result<PathBuf> {
    let directory = value(environment, "CHROMEWEBDRIVER")
        .ok_or_else(|| failure("Missing hosted ChromeDriver directory"))?;
    Ok(Path::new(directory).join(if cfg!(windows) {
        "chromedriver.exe"
    } else {
        "chromedriver"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[cfg(windows)]
    #[test]
    fn windows_runner_mixed_case_driver_variable_finds_its_installed_file() {
        let temporary = tempfile::tempdir().expect("owned runner discovery fixture");
        let directory = temporary.path().join("Selenium WebDrivers مسیر");
        std::fs::create_dir(&directory).expect("owned driver directory");
        let executable = directory.join("chromedriver.exe");
        std::fs::write(&executable, b"controlled discovery fixture").expect("owned driver file");
        // The exact image's Install-Chrome.ps1 registers this mixed-case name.
        let environment = [(
            OsString::from("ChromeWebDriver"),
            directory.into_os_string(),
        )]
        .into_iter()
        .collect();
        let discovered = driver(&environment).expect("installed Windows driver");
        assert_eq!(discovered, executable);
        assert!(discovered.is_file());
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_snapshot_preserves_case_sensitive_driver_lookup() {
        let temporary = tempfile::tempdir().expect("owned Unix driver fixture");
        let mut environment: Environment = [(
            OsString::from("ChromeWebDriver"),
            temporary.path().as_os_str().to_owned(),
        )]
        .into_iter()
        .collect();
        assert!(driver(&environment).is_err());
        environment.insert(
            "CHROMEWEBDRIVER".into(),
            temporary.path().as_os_str().to_owned(),
        );
        assert_eq!(
            driver(&environment).expect("Unix driver"),
            temporary.path().join("chromedriver")
        );
    }
}
