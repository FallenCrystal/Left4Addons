use crate::error::CoreError;
use l4a_vpk::{generate_dummy_vpk, DummySpec};
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use tempfile::{TempDir, TempPath};

/// Stages changes to individual VPKs and retains originals until the caller commits.
#[derive(Default)]
pub(crate) struct FileChanges {
    originals: Vec<Backup>,
    published: Vec<(PathBuf, Metadata)>,
}

pub(crate) struct StagedFile {
    path: PathBuf,
    _directory: TempDir,
}

impl StagedFile {
    pub(crate) fn prepare(src: &Path, dst: &Path) -> Result<Self, CoreError> {
        let directory = staging_dir(dst)?;
        let path = directory.path().join("payload");
        if fs::hard_link(src, &path).is_err() {
            fs::copy(src, &path)?;
        }
        Ok(Self {
            path,
            _directory: directory,
        })
    }
}

struct Backup {
    destination: PathBuf,
    directory: TempDir,
    path: PathBuf,
    metadata: Metadata,
}

impl FileChanges {
    pub(crate) fn run<T>(
        operation: impl FnOnce(&mut Self) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let mut changes = Self::default();
        match operation(&mut changes) {
            Ok(value) => Ok(value),
            Err(error) => Err(changes.rollback(error)),
        }
    }

    pub(crate) fn take(&mut self, path: &Path) -> Result<PathBuf, CoreError> {
        let before = regular_file(path)?;
        let dir = staging_dir(path)?;
        let backup =
            dir.path().join(path.file_name().ok_or_else(|| {
                CoreError::InvalidInput(format!("无效文件路径: {}", path.display()))
            })?);
        fs::rename(path, &backup)?;
        let verified = regular_file(&backup).map(|after| same_file(&before, &after));
        self.originals.push(Backup {
            destination: path.to_owned(),
            directory: dir,
            path: backup.clone(),
            metadata: before,
        });
        if !verified? {
            return Err(CoreError::Conflict(format!(
                "文件已变化: {}",
                path.display()
            )));
        }
        Ok(backup)
    }

    pub(crate) fn publish_copy(&mut self, src: &Path, dst: &Path) -> Result<(), CoreError> {
        self.publish_staged(StagedFile::prepare(src, dst)?, dst)
    }

    pub(crate) fn publish_staged(
        &mut self,
        staged: StagedFile,
        dst: &Path,
    ) -> Result<(), CoreError> {
        self.publish(staged.path, dst)
    }

    pub(crate) fn publish_dummy(
        &mut self,
        src: &Path,
        dst: &Path,
        spec: &DummySpec,
    ) -> Result<(), CoreError> {
        let dir = staging_dir(dst)?;
        let staged = dir.path().join("dummy.vpk");
        generate_dummy_vpk(src, &staged, spec).map_err(CoreError::Vpk)?;
        self.publish(staged, dst)
    }

    fn publish(&mut self, staged: PathBuf, dst: &Path) -> Result<(), CoreError> {
        let metadata = regular_file(&staged)?;
        TempPath::try_from_path(staged)?
            .persist_noclobber(dst)
            .map_err(|error| {
                if error.error.kind() == io::ErrorKind::AlreadyExists {
                    CoreError::Conflict(format!("目标文件已存在: {}", dst.display()))
                } else {
                    error.error.into()
                }
            })?;
        self.published.push((dst.to_owned(), metadata));
        Ok(())
    }

    pub(crate) fn verify(&self) -> Result<(), CoreError> {
        for (path, expected) in self
            .published
            .iter()
            .map(|(path, expected)| (path.as_path(), expected))
            .chain(
                self.originals
                    .iter()
                    .map(|backup| (backup.path.as_path(), &backup.metadata)),
            )
        {
            if !same_file(&regular_file(path)?, expected) {
                return Err(CoreError::Conflict(format!(
                    "文件已变化: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    fn rollback(self, original_error: CoreError) -> CoreError {
        let mut failures = Vec::new();
        for (path, expected) in self.published.into_iter().rev() {
            match fs::symlink_metadata(&path) {
                Ok(actual) if same_file(&actual, &expected) => {
                    if let Err(err) = fs::remove_file(&path) {
                        failures.push(format!("{}: {err}", path.display()));
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                _ => failures.push(format!("文件已被外部修改: {}", path.display())),
            }
        }
        for Backup {
            destination,
            directory,
            path,
            ..
        } in self.originals.into_iter().rev()
        {
            let restored = TempPath::try_from_path(&path).and_then(|mut temp| {
                temp.disable_cleanup(true);
                temp.persist_noclobber(&destination)
                    .map_err(|err| err.error)
            });
            if let Err(err) = restored {
                let _ = directory.keep();
                failures.push(format!(
                    "{} 恢复失败 ({err})，备份保留在 {}",
                    destination.display(),
                    path.display()
                ));
            }
        }
        if failures.is_empty() {
            original_error
        } else {
            CoreError::Conflict(format!("{original_error}；{}", failures.join("；")))
        }
    }
}

pub(crate) fn occupied(path: &Path) -> Result<bool, CoreError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn regular_file(path: &Path) -> Result<Metadata, CoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(CoreError::Conflict(format!(
            "路径不是普通文件: {}",
            path.display()
        )));
    }
    Ok(metadata)
}

pub(crate) fn same_file(a: &Metadata, b: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return false;
        }
    }
    a.is_file() && b.is_file() && a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

fn staging_dir(path: &Path) -> io::Result<TempDir> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    tempfile::Builder::new().prefix(".l4a-").tempdir_in(parent)
}

pub(crate) fn take_matching_dummy(
    changes: &mut FileChanges,
    path: &Path,
    wid: l4a_protocol::WorkshopId,
) -> Result<(), CoreError> {
    let matches = |candidate: &Path| match l4a_vpk::inspect_dummy_vpk(candidate) {
        l4a_vpk::DummyInspection::Dummy(meta) => {
            meta.workshop_id.or_else(|| {
                crate::scanner::extract_workshop_id_from_filename(
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                )
            }) == Some(wid)
        }
        _ => false,
    };
    if occupied(path)? {
        regular_file(path)?;
        let conflict = || CoreError::Conflict(format!("目标路径已有其他文件: {}", path.display()));
        if !matches(path) {
            return Err(conflict());
        }
        let backup = changes.take(path)?;
        if !matches(&backup) {
            return Err(conflict());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn same_filesystem_moves_reuse_the_source_inode() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.vpk");
        let destination = dir.path().join("destination.vpk");
        fs::write(&source, b"VPK payload").unwrap();
        let inode = fs::metadata(&source).unwrap().ino();
        FileChanges::run(|changes| {
            let backup = changes.take(&source)?;
            changes.publish_copy(&backup, &destination)?;
            changes.verify()
        })
        .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::metadata(&destination).unwrap().ino(), inode);
        assert_eq!(fs::read(destination).unwrap(), b"VPK payload");
    }

    #[test]
    fn rollback_keeps_a_backup_when_an_external_file_occupies_the_original_path() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("addon.vpk");
        fs::write(&original, b"original").unwrap();
        let mut recovery = PathBuf::new();
        let result = FileChanges::run(|changes| {
            recovery = changes.take(&original)?;
            fs::write(&original, b"external update")?;
            Err::<(), _>(CoreError::Conflict("test failure".into()))
        });
        let message = result.unwrap_err().to_string();
        assert!(message.contains(recovery.to_str().unwrap()));
        assert_eq!(fs::read(original).unwrap(), b"external update");
        assert_eq!(fs::read(recovery).unwrap(), b"original");
    }
}
