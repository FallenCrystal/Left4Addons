use crate::config::AppPaths;
use crate::error::CoreError;
use l4a_protocol::{FilterTokenStatus, LlmTokenSource};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::hkdf::{KeyType, Salt, HKDF_SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

const INNER_MAGIC: &[u8; 8] = b"L4ATOK01";
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;

#[cfg(windows)]
const DPAPI_MAGIC: &[u8; 8] = b"L4ADPA01";
#[cfg(windows)]
const DPAPI_KEY_MAGIC: &[u8; 8] = b"L4AWVK01";

struct DerivedKeyLen;

impl KeyType for DerivedKeyLen {
    fn len(&self) -> usize {
        KEY_LEN
    }
}

pub fn validate_env_var_name(name: &str) -> Result<&str, CoreError> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return Err(CoreError::InvalidInput(
            "环境变量名称长度必须在 1..=128 之间".into(),
        ));
    }
    let first = bytes[0];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return Err(CoreError::InvalidInput(
            "环境变量名称必须以英文字母或下划线开头".into(),
        ));
    }
    if !bytes[1..]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        return Err(CoreError::InvalidInput(
            "环境变量名称仅允许包含英文字母、数字和下划线".into(),
        ));
    }
    Ok(name)
}

const MAX_WIPE_BYTES: u64 = 16_384;

#[derive(Debug, Clone)]
pub struct TokenVault {
    sealed_path: PathBuf,
    vault_key_path: Result<PathBuf, String>,
    identity_override: Option<(String, String)>,
}

impl TokenVault {
    pub fn default_for_paths(paths: &AppPaths) -> Self {
        let vault_key_path = match default_user_vault_key_path(&paths.runtime_dir) {
            Ok(path) => Ok(path),
            Err(CoreError::Config(msg)) => Err(msg),
            Err(other) => Err(other.to_string()),
        };
        Self {
            sealed_path: paths.llm_token_path.clone(),
            vault_key_path,
            identity_override: None,
        }
    }

    pub fn with_custom_identity(
        sealed_path: impl Into<PathBuf>,
        vault_key_path: impl Into<PathBuf>,
        machine_id: impl Into<String>,
        user_id: impl Into<String>,
    ) -> Self {
        Self {
            sealed_path: sealed_path.into(),
            vault_key_path: Ok(vault_key_path.into()),
            identity_override: Some((machine_id.into(), user_id.into())),
        }
    }

    pub fn sealed_path(&self) -> &Path {
        &self.sealed_path
    }

    pub fn vault_key_path(&self) -> Result<&Path, CoreError> {
        self.vault_key_path
            .as_deref()
            .map_err(|msg| CoreError::Config(msg.clone()))
    }

    pub fn has_sealed_token(&self) -> bool {
        fs::symlink_metadata(&self.sealed_path)
            .is_ok_and(|meta| meta.is_file() && meta.len() > (INNER_MAGIC.len() + NONCE_LEN) as u64)
    }

    pub fn status(&self, env_var_name: &str) -> FilterTokenStatus {
        let trimmed = env_var_name.trim();
        let env_var_present = validate_env_var_name(trimmed).is_ok()
            && std::env::var(trimmed)
                .map(Zeroizing::new)
                .is_ok_and(|val| !val.trim().is_empty());
        FilterTokenStatus {
            cache_generation: 0,
            has_encrypted_token: self.has_sealed_token(),
            env_var_name: trimmed.to_string(),
            env_var_present,
        }
    }

    pub fn clear(&self) -> Result<(), CoreError> {
        if let Ok(meta) = fs::symlink_metadata(&self.sealed_path) {
            if meta.is_file() {
                let len = meta.len().min(MAX_WIPE_BYTES) as usize;
                if len > 0 {
                    if let Ok(mut f) = fs::OpenOptions::new().write(true).open(&self.sealed_path) {
                        let zeros = Zeroizing::new(vec![0u8; len]);
                        let _ = f.write_all(&zeros);
                        let _ = f.sync_all();
                    }
                }
            }
            match fs::remove_file(&self.sealed_path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(CoreError::Io(err)),
            }
        }
        Ok(())
    }

    pub fn seal(&self, plaintext_token: &str) -> Result<(), CoreError> {
        let clean = Zeroizing::new(plaintext_token.trim().to_string());
        if clean.is_empty() {
            return Err(CoreError::InvalidInput("API Token 不能为空".into()));
        }
        if clean.len() > 8192 {
            return Err(CoreError::InvalidInput("API Token 长度超出限制".into()));
        }

        let (machine_id, user_id) = self.resolve_identity()?;
        let vault_key = self.load_or_create_vault_key(&machine_id, &user_id)?;
        let derived_key = derive_aead_key(&vault_key, &machine_id)?;

        let rng = SystemRandom::new();
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| CoreError::Config("系统随机数生成器不可用".into()))?;

        let aad_str = build_aad_string(&machine_id, &user_id);
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &*derived_key)
            .map_err(|_| CoreError::Config("初始化 AEAD 密钥失败".into()))?;
        let key = LessSafeKey::new(unbound);

        let mut in_out = Zeroizing::new(Vec::with_capacity(clean.len() + TAG_LEN));
        in_out.extend_from_slice(clean.as_bytes());
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad_str.as_bytes()),
            &mut *in_out,
        )
        .map_err(|_| CoreError::Config("加密 API Token 失败".into()))?;

        let mut inner = Zeroizing::new(Vec::with_capacity(
            INNER_MAGIC.len() + NONCE_LEN + in_out.len(),
        ));
        inner.extend_from_slice(INNER_MAGIC);
        inner.extend_from_slice(&nonce_bytes);
        inner.extend_from_slice(&in_out);

        #[cfg(windows)]
        let outer_bytes = {
            let entropy = dpapi_entropy(&machine_id, &user_id);
            let dpapi_blob = dpapi::protect(&inner, &entropy)?;
            let mut buf = Vec::with_capacity(DPAPI_MAGIC.len() + dpapi_blob.len());
            buf.extend_from_slice(DPAPI_MAGIC);
            buf.extend_from_slice(&dpapi_blob);
            buf
        };

        #[cfg(not(windows))]
        let outer_bytes = inner.to_vec();

        write_private_file(&self.sealed_path, &outer_bytes, false)?;
        Ok(())
    }

    pub fn unseal(&self) -> Result<Zeroizing<String>, CoreError> {
        let raw = fs::read(&self.sealed_path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                CoreError::NotFound("尚未配置本地加密的 LLM API Token".into())
            }
            _ => CoreError::Io(err),
        })?;

        let (machine_id, user_id) = self.resolve_identity()?;

        #[cfg(windows)]
        let inner: Zeroizing<Vec<u8>> = {
            if raw.len() <= DPAPI_MAGIC.len() || &raw[..DPAPI_MAGIC.len()] != DPAPI_MAGIC {
                return Err(CoreError::Config(
                    "本地加密 Token 格式无效或不属于当前平台".into(),
                ));
            }
            let entropy = dpapi_entropy(&machine_id, &user_id);
            dpapi::unprotect(&raw[DPAPI_MAGIC.len()..], &entropy)?
        };

        #[cfg(not(windows))]
        let inner: Zeroizing<Vec<u8>> = Zeroizing::new(raw);

        if inner.len() < INNER_MAGIC.len() + NONCE_LEN + TAG_LEN + 1
            || &inner[..INNER_MAGIC.len()] != INNER_MAGIC
        {
            return Err(CoreError::Config(
                "本地加密 Token 文件已损坏或格式不受支持".into(),
            ));
        }

        let vault_key = self.read_existing_vault_key(&machine_id, &user_id)?;
        let derived_key = derive_aead_key(&vault_key, &machine_id)?;

        let nonce_start = INNER_MAGIC.len();
        let cipher_start = nonce_start + NONCE_LEN;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(&inner[nonce_start..cipher_start]);

        let aad_str = build_aad_string(&machine_id, &user_id);
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &*derived_key)
            .map_err(|_| CoreError::Config("初始化解密密钥失败".into()))?;
        let key = LessSafeKey::new(unbound);

        let mut cipher_buf = Zeroizing::new(inner[cipher_start..].to_vec());
        let plain_slice = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad_str.as_bytes()),
                &mut cipher_buf,
            )
            .map_err(|_| {
                CoreError::Config(
                    "无法解密本地 API Token（仅允许加密该凭据的当前用户与计算机解密）".into(),
                )
            })?;

        let parsed = std::str::from_utf8(plain_slice)
            .map_err(|_| CoreError::Config("解密后的 Token 不是有效的 UTF-8 文本".into()))?;
        let out = Zeroizing::new(parsed.to_string());
        plain_slice.zeroize();
        if out.trim().is_empty() {
            return Err(CoreError::Config("本地加密的 API Token 为空".into()));
        }
        Ok(out)
    }

    pub fn resolve_token(
        &self,
        source: LlmTokenSource,
        env_var_name: &str,
    ) -> Result<Zeroizing<String>, CoreError> {
        match source {
            LlmTokenSource::EnvVar => {
                let name = validate_env_var_name(env_var_name.trim())?;
                let raw = std::env::var(name).map(Zeroizing::new).map_err(|_| {
                    CoreError::Config(format!("未检测到 LLM Token 环境变量 {name}"))
                })?;
                let trimmed = Zeroizing::new(raw.trim().to_string());
                if trimmed.is_empty() {
                    return Err(CoreError::Config(format!(
                        "LLM Token 环境变量 {name} 的值为空"
                    )));
                }
                Ok(trimmed)
            }
            LlmTokenSource::EncryptedLocal => self.unseal(),
        }
    }

    fn resolve_identity(&self) -> Result<(String, String), CoreError> {
        if let Some((ref m, ref u)) = self.identity_override {
            if m.trim().is_empty() || u.trim().is_empty() {
                return Err(CoreError::Config("本机或当前用户身份标识为空".into()));
            }
            return Ok((m.clone(), u.clone()));
        }
        let machine_id = detect_os_machine_id()?;
        let user_id = detect_os_user_id()?;
        Ok((machine_id, user_id))
    }

    fn load_or_create_vault_key(
        &self,
        machine_id: &str,
        user_id: &str,
    ) -> Result<Zeroizing<[u8; KEY_LEN]>, CoreError> {
        let key_path = self.vault_key_path()?;
        match fs::symlink_metadata(key_path) {
            Ok(_) => return self.read_existing_vault_key(machine_id, user_id),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(CoreError::Io(err)),
        }

        let rng = SystemRandom::new();
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        rng.fill(&mut *key)
            .map_err(|_| CoreError::Config("生成本地主密钥随机数失败".into()))?;

        #[cfg(windows)]
        {
            let entropy = dpapi_entropy(machine_id, user_id);
            let protected = dpapi::protect(&*key, &entropy)?;
            let mut blob = Vec::with_capacity(DPAPI_KEY_MAGIC.len() + protected.len());
            blob.extend_from_slice(DPAPI_KEY_MAGIC);
            blob.extend_from_slice(&protected);
            write_private_file(key_path, &blob, true)?;
        }

        #[cfg(not(windows))]
        {
            let _ = (machine_id, user_id);
            write_private_file(key_path, &*key, true)?;
        }

        Ok(key)
    }

    fn read_existing_vault_key(
        &self,
        machine_id: &str,
        user_id: &str,
    ) -> Result<Zeroizing<[u8; KEY_LEN]>, CoreError> {
        let key_path = self.vault_key_path()?;
        verify_existing_vault_key_metadata(key_path)?;

        let raw = Zeroizing::new(fs::read(key_path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                CoreError::Config("缺少当前用户专属密钥文件，无法解密本地 API Token".into())
            }
            _ => CoreError::Io(err),
        })?);

        #[cfg(windows)]
        {
            if raw.len() <= DPAPI_KEY_MAGIC.len()
                || &raw[..DPAPI_KEY_MAGIC.len()] != DPAPI_KEY_MAGIC
            {
                return Err(CoreError::Config(
                    "当前用户密钥文件缺少 DPAPI 保护头或已损坏".into(),
                ));
            }
            let entropy = dpapi_entropy(machine_id, user_id);
            let plain = dpapi::unprotect(&raw[DPAPI_KEY_MAGIC.len()..], &entropy)?;
            if plain.len() != KEY_LEN {
                return Err(CoreError::Config("当前用户密钥长度异常".into()));
            }
            let mut key = Zeroizing::new([0u8; KEY_LEN]);
            key.copy_from_slice(&plain);
            Ok(key)
        }

        #[cfg(not(windows))]
        {
            let _ = (machine_id, user_id);
            if raw.len() != KEY_LEN {
                return Err(CoreError::Config("当前用户密钥文件已损坏".into()));
            }
            let mut key = Zeroizing::new([0u8; KEY_LEN]);
            key.copy_from_slice(&raw);
            Ok(key)
        }
    }
}

fn verify_existing_vault_key_metadata(path: &Path) -> Result<(), CoreError> {
    let meta = fs::symlink_metadata(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => {
            CoreError::Config("缺少当前用户专属密钥文件，无法解密本地 API Token".into())
        }
        _ => CoreError::Io(err),
    })?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(CoreError::Config(
            "当前用户密钥文件类型异常（拒绝符号链接或非普通文件）".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 {
            return Err(CoreError::Config(
                "当前用户密钥文件权限过宽（必须为 0600 仅当前用户可读写）".into(),
            ));
        }
        if let Some(euid) = detect_current_unix_euid() {
            if meta.uid() != euid {
                return Err(CoreError::Config(
                    "当前用户密钥文件所有者与当前进程有效用户不一致".into(),
                ));
            }
        }
    }
    Ok(())
}

fn derive_aead_key(
    vault_key: &[u8; KEY_LEN],
    machine_id: &str,
) -> Result<Zeroizing<[u8; KEY_LEN]>, CoreError> {
    let machine_hash = Sha256::digest(machine_id.as_bytes());
    let salt = Salt::new(HKDF_SHA256, machine_hash.as_slice());
    let prk = salt.extract(vault_key);
    let okm = prk
        .expand(&[b"left4addons-llm-token-v1"], DerivedKeyLen)
        .map_err(|_| CoreError::Config("派生加密密钥失败".into()))?;
    let mut derived = Zeroizing::new([0u8; KEY_LEN]);
    okm.fill(&mut *derived)
        .map_err(|_| CoreError::Config("填充派生加密密钥失败".into()))?;
    Ok(derived)
}

fn build_aad_string(machine_id: &str, user_id: &str) -> String {
    let machine_hash = hex::encode(Sha256::digest(machine_id.as_bytes()));
    format!("l4a-token-v1:{user_id}:{machine_hash}")
}

#[cfg(windows)]
fn dpapi_entropy(machine_id: &str, user_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"l4a-dpapi-v1:");
    hasher.update(machine_id.as_bytes());
    hasher.update(b":");
    hasher.update(user_id.as_bytes());
    hasher.finalize().into()
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_vault_key_path_with_base(runtime_dir: &Path, base_dir: &Path) -> PathBuf {
    let norm_rt = normalize_path_lexically(runtime_dir);
    let norm_base = normalize_path_lexically(base_dir);
    if norm_base.starts_with(&norm_rt) || norm_rt.starts_with(&norm_base) {
        let anchor = if norm_base.starts_with(&norm_rt) {
            &norm_rt
        } else {
            &norm_base
        };
        return anchor
            .parent()
            .unwrap_or(anchor)
            .join(".left4addons-vault")
            .join("vault.key");
    }
    norm_base.join("vault.key")
}

fn default_user_vault_key_path(runtime_dir: &Path) -> Result<PathBuf, CoreError> {
    let base_dir = os_user_private_config_dir()?;
    Ok(resolve_vault_key_key_dir(runtime_dir, &base_dir))
}

fn resolve_vault_key_key_dir(runtime_dir: &Path, base_dir: &Path) -> PathBuf {
    resolve_vault_key_path_with_base(runtime_dir, base_dir)
}

fn os_user_private_config_dir() -> Result<PathBuf, CoreError> {
    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .filter(|v| !v.is_empty())
        {
            return Ok(PathBuf::from(local_app_data).join("Left4Addons"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("Left4Addons"));
        }
        Err(CoreError::Config(
            "无法确定当前用户的私有配置目录 (LOCALAPPDATA / USERPROFILE)".into(),
        ))
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            let p = PathBuf::from(home);
            if p.is_absolute() {
                return Ok(p
                    .join("Library")
                    .join("Application Support")
                    .join("Left4Addons"));
            }
        }
        Err(CoreError::Config(
            "无法确定当前用户的私有配置目录 (HOME)".into(),
        ))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        unix_private_config_dir_from_vars(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn unix_private_config_dir_from_vars(
    xdg_config_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, CoreError> {
    if let Some(xdg) = xdg_config_home.filter(|v| !v.is_empty()) {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Ok(p.join("left4addons"));
        }
    }
    if let Some(h) = home.filter(|v| !v.is_empty()) {
        let p = PathBuf::from(h);
        if p.is_absolute() {
            return Ok(p.join(".config").join("left4addons"));
        }
    }
    Err(CoreError::Config(
        "无法确定当前用户的私有配置目录 (XDG_CONFIG_HOME / HOME)".into(),
    ))
}

fn detect_os_machine_id() -> Result<String, CoreError> {
    #[cfg(target_os = "linux")]
    {
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(raw) = fs::read_to_string(path) {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    return Ok(trimmed.to_string());
                }
            }
        }
        if let Ok(raw) = fs::read_to_string("/proc/sys/kernel/hostname") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Ok(format!("linux-host:{trimmed}"));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    if let Some((_, rhs)) = line.split_once("IOPlatformUUID") {
                        if let Some(val) = rhs.split('"').nth(1) {
                            let trimmed = val.trim();
                            if !trimmed.is_empty() {
                                return Ok(trimmed.to_string());
                            }
                        }
                    }
                }
            }
        }
        if let Ok(out) = std::process::Command::new("hostname").output() {
            if out.status.success() {
                let host = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !host.is_empty() {
                    return Ok(format!("macos-host:{host}"));
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(comp) = std::env::var("COMPUTERNAME") {
            let trimmed = comp.trim();
            if !trimmed.is_empty() {
                return Ok(format!("win-host:{trimmed}"));
            }
        }
    }

    if let Ok(host) = std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")) {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            return Ok(format!("host:{trimmed}"));
        }
    }

    Err(CoreError::Config("无法识别当前计算机唯一标识".into()))
}

fn detect_os_user_id() -> Result<String, CoreError> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Uid:") {
                    // Format: Real, Effective, Saved set, Filesystem UIDs
                    let mut parts = rest.split_whitespace();
                    let real = parts.next();
                    let effective = parts.next().or(real);
                    if let Some(euid) = effective {
                        if !euid.is_empty() {
                            return Ok(format!("uid:{euid}"));
                        }
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("id").arg("-u").output() {
            if out.status.success() {
                let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !uid.is_empty() {
                    return Ok(format!("uid:{uid}"));
                }
            }
        }
        if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")) {
            let trimmed = user.trim();
            if !trimmed.is_empty() {
                return Ok(format!("user:{trimmed}"));
            }
        }
    }

    #[cfg(windows)]
    {
        let domain = std::env::var("USERDOMAIN").unwrap_or_default();
        if let Ok(user) = std::env::var("USERNAME") {
            let trimmed = user.trim();
            if !trimmed.is_empty() {
                return Ok(format!("win-user:{}\\{}", domain.trim(), trimmed));
            }
        }
    }

    Err(CoreError::Config("无法识别当前系统用户标识".into()))
}

#[cfg(unix)]
fn detect_current_unix_euid() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Uid:") {
                    let mut parts = rest.split_whitespace();
                    let real = parts.next();
                    let effective = parts.next().or(real);
                    if let Some(euid_str) = effective {
                        if let Ok(parsed) = euid_str.parse::<u32>() {
                            return Some(parsed);
                        }
                    }
                }
            }
        }
    }

    if let Ok(out) = std::process::Command::new("id").arg("-u").output() {
        if out.status.success() {
            let uid_str = String::from_utf8_lossy(&out.stdout);
            if let Ok(parsed) = uid_str.trim().parse::<u32>() {
                return Some(parsed);
            }
        }
    }
    None
}

fn write_private_file(
    path: &Path,
    contents: &[u8],
    enforce_private_dir: bool,
) -> Result<(), CoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::InvalidInput("无效的密钥存储路径".into()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        if enforce_private_dir {
            builder.mode(0o700);
        }
        builder.create(parent)?;
        if enforce_private_dir {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        let rng = SystemRandom::new();
        let mut suffix = [0u8; 8];
        let _ = rng.fill(&mut suffix);
        let tmp_path = parent.join(format!(
            ".tmp_{}_{}",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("secret"),
            hex::encode(suffix)
        ));

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        if let Err(err) = file.write_all(contents).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&tmp_path);
            return Err(CoreError::Io(err));
        }
        drop(file);
        if let Err(err) = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&tmp_path);
            return Err(CoreError::Io(err));
        }
        if let Err(err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(CoreError::Io(err));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = enforce_private_dir;
        fs::create_dir_all(parent)?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".l4a_secret_")
            .tempfile_in(parent)?;
        tmp.write_all(contents)?;
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| CoreError::Io(e.error))?;
        Ok(())
    }
}

#[cfg(windows)]
mod dpapi {
    use crate::error::CoreError;
    use std::ffi::c_void;
    use std::ptr;
    use zeroize::Zeroizing;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;

        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    pub fn protect(plain: &[u8], entropy: &[u8]) -> Result<Vec<u8>, CoreError> {
        let in_blob = DataBlob {
            cb_data: u32::try_from(plain.len())
                .map_err(|_| CoreError::InvalidInput("待加密数据过大".into()))?,
            pb_data: plain.as_ptr() as *mut u8,
        };
        let entropy_blob = DataBlob {
            cb_data: u32::try_from(entropy.len())
                .map_err(|_| CoreError::InvalidInput("熵数据过大".into()))?,
            pb_data: entropy.as_ptr() as *mut u8,
        };
        let mut out_blob = DataBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };

        // SAFETY: Valid DataBlob pointers passed to Win32 CryptProtectData; out_blob freed with LocalFree.
        let ok = unsafe {
            CryptProtectData(
                &in_blob,
                ptr::null(),
                &entropy_blob,
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };
        if ok == 0 || out_blob.pb_data.is_null() {
            return Err(CoreError::Config("Windows DPAPI 加密本地凭据失败".into()));
        }

        // SAFETY: out_blob.pb_data points to out_blob.cb_data bytes allocated by CryptProtectData.
        let result = unsafe {
            let slice = std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize);
            let vec = slice.to_vec();
            LocalFree(out_blob.pb_data as *mut c_void);
            vec
        };
        Ok(result)
    }

    pub fn unprotect(cipher: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, CoreError> {
        let in_blob = DataBlob {
            cb_data: u32::try_from(cipher.len())
                .map_err(|_| CoreError::InvalidInput("加密数据过大".into()))?,
            pb_data: cipher.as_ptr() as *mut u8,
        };
        let entropy_blob = DataBlob {
            cb_data: u32::try_from(entropy.len())
                .map_err(|_| CoreError::InvalidInput("熵数据过大".into()))?,
            pb_data: entropy.as_ptr() as *mut u8,
        };
        let mut out_blob = DataBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };

        // SAFETY: Valid DataBlob pointers passed to Win32 CryptUnprotectData; out_blob zeroized then freed with LocalFree.
        let ok = unsafe {
            CryptUnprotectData(
                &in_blob,
                ptr::null_mut(),
                &entropy_blob,
                ptr::null_mut(),
                ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };
        if ok == 0 || out_blob.pb_data.is_null() {
            return Err(CoreError::Config(
                "Windows DPAPI 解密失败（仅当前系统用户可解密）".into(),
            ));
        }

        // SAFETY: out_blob.pb_data points to out_blob.cb_data bytes allocated by CryptUnprotectData.
        let out = unsafe {
            let slice = std::slice::from_raw_parts_mut(out_blob.pb_data, out_blob.cb_data as usize);
            let vec = Zeroizing::new(slice.to_vec());
            for byte in slice.iter_mut() {
                ptr::write_volatile(byte, 0);
            }
            LocalFree(out_blob.pb_data as *mut c_void);
            vec
        };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validates_env_var_names_strictly() {
        assert!(validate_env_var_name("L4A_LLM_API_KEY").is_ok());
        assert!(validate_env_var_name("_OPENAI_KEY_1").is_ok());
        for bad in ["", "1ABC", "BAD-NAME", "BAD NAME", "A=B", "A\0B"] {
            assert!(validate_env_var_name(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn custom_identity_roundtrip_and_isolation_checks() {
        let dir = tempdir().unwrap();
        let sealed = dir.path().join("config").join("llm_token.sealed");
        let key1 = dir.path().join("user1").join("vault.key");
        let key2 = dir.path().join("user2").join("vault.key");

        let vault = TokenVault::with_custom_identity(&sealed, &key1, "machine-alpha", "uid:1000");
        assert!(!vault.has_sealed_token());

        vault.seal("  sk-test-super-secret-999  ").unwrap();
        assert!(vault.has_sealed_token());
        assert_eq!(vault.unseal().unwrap().as_str(), "sk-test-super-secret-999");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let sealed_mode = fs::metadata(&sealed).unwrap().permissions().mode() & 0o777;
            let key_mode = fs::metadata(&key1).unwrap().permissions().mode() & 0o777;
            assert_eq!(sealed_mode, 0o600);
            assert_eq!(key_mode, 0o600);

            // Group/world-readable vault.key must be rejected!
            fs::set_permissions(&key1, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(
                vault.unseal().is_err(),
                "must reject vault.key with 0o077 bits set"
            );
            fs::set_permissions(&key1, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(vault.unseal().unwrap().as_str(), "sk-test-super-secret-999");

            // Symlinked vault.key must be rejected!
            let symlink_key = dir.path().join("symlink_vault.key");
            std::os::unix::fs::symlink(&key1, &symlink_key).unwrap();
            let symlink_vault = TokenVault::with_custom_identity(
                &sealed,
                &symlink_key,
                "machine-alpha",
                "uid:1000",
            );
            assert!(
                symlink_vault.unseal().is_err(),
                "must reject symlinked vault.key"
            );
        }

        // Wrong machine_id must fail
        let wrong_machine =
            TokenVault::with_custom_identity(&sealed, &key1, "machine-beta", "uid:1000");
        assert!(wrong_machine.unseal().is_err());

        // Wrong user_id must fail
        let wrong_user =
            TokenVault::with_custom_identity(&sealed, &key1, "machine-alpha", "uid:1001");
        assert!(wrong_user.unseal().is_err());

        // Wrong vault_key must fail
        let wrong_key =
            TokenVault::with_custom_identity(&sealed, &key2, "machine-alpha", "uid:1000");
        wrong_key
            .load_or_create_vault_key("machine-alpha", "uid:1000")
            .unwrap();
        assert!(wrong_key.unseal().is_err());

        // Tampered ciphertext must fail
        let mut bytes = fs::read(&sealed).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x55;
        fs::write(&sealed, &bytes).unwrap();
        let err = vault.unseal().unwrap_err().to_string();
        assert!(!err.contains("sk-test-super-secret-999"));
        assert!(vault.unseal().is_err());

        // Large token (>4096 bytes, up to 8192) roundtrip and clear()
        let long_token = format!("sk-{}", "a".repeat(7000));
        vault.seal(&long_token).unwrap();
        assert!(fs::metadata(&sealed).unwrap().len() > 7000);
        assert_eq!(vault.unseal().unwrap().as_str(), long_token);

        vault.clear().unwrap();
        assert!(!vault.has_sealed_token());
    }

    #[test]
    fn lexical_vault_key_path_is_identical_before_and_after_dir_creation() {
        let dir = tempdir().unwrap();
        let rt_dir = dir.path().join("workspace").join("..").join("runtime");
        let overlapping_base = dir.path().join("runtime").join("left4addons");

        // Run #1: neither rt_dir nor overlapping_base exists on disk yet
        let run1 = resolve_vault_key_path_with_base(&rt_dir, &overlapping_base);

        // Create directories as Run #1 would during seal()
        fs::create_dir_all(&overlapping_base).unwrap();
        fs::create_dir_all(run1.parent().unwrap()).unwrap();

        // Run #2: directories now exist on disk — path must be 100% identical!
        let run2 = resolve_vault_key_path_with_base(&rt_dir, &overlapping_base);
        assert_eq!(run1, run2);
        assert!(!run1.starts_with(dir.path().join("runtime")));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn missing_user_config_env_returns_error_instead_of_tmp_fallback() {
        let res = unix_private_config_dir_from_vars(None, None);
        assert!(matches!(res, Err(CoreError::Config(_))));
        let rel_res = unix_private_config_dir_from_vars(
            Some(std::ffi::OsStr::new("relative/xdg")),
            Some(std::ffi::OsStr::new("")),
        );
        assert!(matches!(rel_res, Err(CoreError::Config(_))));
    }
}
