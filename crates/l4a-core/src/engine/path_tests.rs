use super::*;
use tempfile::tempdir;

#[test]
fn resolves_existing_paths_only_inside_managed_directories() {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    let workshop = dir.path().join("workshop");
    let outside = dir.path().join("addons-other");
    for path in [&loading, &workshop, &outside] {
        fs::create_dir(path).unwrap();
    }
    let loading_file = loading.join("local.vpk");
    let workshop_file = workshop.join("123456.vpk");
    let outside_file = outside.join("outside.vpk");
    for path in [&loading_file, &workshop_file, &outside_file] {
        fs::write(path, b"test").unwrap();
    }
    let paths = AppPaths::from_runtime_dir(dir.path().join("runtime"));
    paths.ensure_dirs().unwrap();
    save_config(
        &paths.settings_path,
        &AppConfig {
            loading_dir: loading.to_str().unwrap().into(),
            workshop_dir: workshop.to_str().unwrap().into(),
            ..Default::default()
        },
    )
    .unwrap();
    let engine = L4aEngine::open_default(&paths.runtime_dir, Arc::new(NullEventSink)).unwrap();
    for path in [&loading, &workshop, &loading_file, &workshop_file] {
        assert_eq!(
            engine.resolve_managed_path(path.to_str().unwrap()).unwrap(),
            path.canonicalize().unwrap()
        );
    }
    for bad in ["", " \t", "nul\0path"] {
        assert!(matches!(
            engine.resolve_managed_path(bad),
            Err(CoreError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        engine.resolve_managed_path(loading.join("missing.vpk").to_str().unwrap()),
        Err(CoreError::NotFound(_))
    ));
    for path in [
        outside_file.clone(),
        loading.join("../addons-other/outside.vpk"),
    ] {
        assert!(matches!(
            engine.resolve_managed_path(path.to_str().unwrap()),
            Err(CoreError::InvalidInput(_))
        ));
    }
    #[cfg(unix)]
    {
        let link = loading.join("outside-link.vpk");
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();
        assert!(matches!(
            engine.resolve_managed_path(link.to_str().unwrap()),
            Err(CoreError::InvalidInput(_))
        ));
    }

    fs::remove_dir_all(workshop).unwrap();
    assert!(engine
        .resolve_managed_path(loading.to_str().unwrap())
        .is_ok());
    fs::remove_dir_all(loading).unwrap();
    assert!(matches!(
        engine.resolve_managed_path(outside_file.to_str().unwrap()),
        Err(CoreError::NotFound(_))
    ));
}
