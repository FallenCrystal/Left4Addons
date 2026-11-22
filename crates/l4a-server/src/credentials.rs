use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use steam_vent::auth::GuardDataStore;

pub(crate) fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o600));
    }
    let mut temporary = builder.prefix(".l4a-credentials-").tempfile_in(parent)?;
    restrict_permissions(temporary.as_file())?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|err| err.error)?;
    Ok(())
}

fn restrict_permissions(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata()?.permissions().mode() & 0o777 != 0o600 {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            if file.metadata()?.permissions().mode() & 0o777 != 0o600 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "could not restrict credential file permissions",
                ));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

pub(crate) struct SteamGuardFile {
    pub path: PathBuf,
    pub lock: Arc<Mutex<()>>,
}

impl SteamGuardFile {
    fn read_tokens(&self) -> io::Result<HashMap<String, String>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(err) => return Err(err),
        };
        restrict_permissions(&file)?;
        let mut raw = String::new();
        file.read_to_string(&mut raw)?;
        serde_json::from_str(&raw).map_err(io::Error::other)
    }
}

impl GuardDataStore for SteamGuardFile {
    type Err = io::Error;

    async fn load(&mut self, account: &str) -> io::Result<Option<String>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("Steam Guard file lock poisoned"))?;
        Ok(self
            .read_tokens()?
            .remove(account)
            .filter(|token| !token.is_empty()))
    }

    async fn store(&mut self, account: &str, machine_token: String) -> io::Result<()> {
        if machine_token.is_empty() {
            return Ok(());
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| io::Error::other("Steam Guard file lock poisoned"))?;
        let mut tokens = self.read_tokens()?;
        tokens.insert(account.to_owned(), machine_token);
        let contents = serde_json::to_vec(&tokens).map_err(io::Error::other)?;
        write_private_file(&self.path, &contents)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn guard_file_is_private_and_keeps_the_existing_token_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steam-guard.json");
        fs::write(&path, br#"{"existing":"old-token"}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let mut store = SteamGuardFile {
            path: path.clone(),
            lock: Arc::new(Mutex::new(())),
        };
        assert_eq!(
            store.load("existing").await.unwrap().as_deref(),
            Some("old-token")
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        store.store("new", "fixture-token".into()).await.unwrap();
        assert_eq!(
            store.load("new").await.unwrap().as_deref(),
            Some("fixture-token")
        );
        assert_eq!(
            store.load("existing").await.unwrap().as_deref(),
            Some("old-token")
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
