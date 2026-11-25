use super::*;

#[test]
fn gn_paths_and_casefold_invariants() -> Result<()> {
    assert_eq!(
        runtime_dependency_path("./locales//فارسی.pak")?,
        "locales/فارسی.pak"
    );
    for path in ["../asset", "/asset", "C:asset", "sub\\asset", "", "./"] {
        assert!(runtime_dependency_path(path).is_err(), "{path:?}");
    }
    #[cfg(target_os = "linux")]
    {
        let directory = tempfile::tempdir()?;
        fs::create_dir(directory.path().join("Locales"))?;
        fs::create_dir(directory.path().join("locales"))?;
        fs::write(directory.path().join("Locales/a.pak"), b"a")?;
        fs::write(directory.path().join("locales/b.pak"), b"b")?;
        let inputs = BTreeSet::from(["Locales".to_owned(), "locales".to_owned()]);
        assert!(resolve_runtime_files(directory.path(), &inputs).is_err());
    }
    Ok(())
}

#[test]
fn round_trips_persian_runtime_and_rejects_changed_payload() -> Result<()> {
    let root = tempfile::tempdir()?;
    let name = "locales/داده‌های مرورگر.pak";
    let source = root.path().join("native");
    fs::create_dir_all(source.join("locales"))?;
    fs::write(source.join(name), b"data")?;
    let inventory = Inventory {
        files: BTreeMap::from([(
            name.to_owned(),
            FileRecord {
                sha256: sha256(&source.join(name))?,
                bytes: 4,
            },
        )]),
        bytes: 4,
        gn_sha256: "unused in test".into(),
        gn_targets: ["chrome".into(), "chromedriver".into()],
        publisher_signing: "NOT_CONFIGURED".into(),
    };
    let package = root.path().join("fixture.tar.gz");
    let extraction = root.path().join("Numera مسیر");
    write_archive(&source, &package, &inventory)?;
    extract_archive(&package, &extraction, &inventory)?;
    assert_eq!(fs::read(extraction.join("runtime").join(name))?, b"data");
    fs::write(source.join(name), b"same")?;
    let changed = root.path().join("changed.tar.gz");
    write_archive(&source, &changed, &inventory)?;
    assert!(extract_archive(&changed, &root.path().join("changed"), &inventory).is_err());
    Ok(())
}
