use crate::error::CoreError;
use std::env;
use windows_sys::core::PCWSTR;
use windows_sys::w;
use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ, RRF_NOEXPAND,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RRF_ZEROONFAILURE,
};
use zeroize::Zeroizing;

// The Windows environment variable length limit, including the terminating NUL.
const MAX_VALUE_UNITS: usize = 32_768;
// Bound registry lookups when references form a cycle or branch repeatedly.
const MAX_REFERENCES: usize = 256;
type RegistryLocation = (HKEY, PCWSTR);

pub(super) fn read(name: &str) -> Result<Option<Zeroizing<String>>, CoreError> {
    read_from(
        name,
        &[
            (HKEY_CURRENT_USER, w!("Environment")),
            (
                HKEY_LOCAL_MACHINE,
                w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
            ),
        ],
    )
}

fn read_from(
    name: &str,
    locations: &[RegistryLocation],
) -> Result<Option<Zeroizing<String>>, CoreError> {
    let mut references_left = MAX_REFERENCES;
    resolve_value(name, locations, &mut references_left)
        .map_err(|()| CoreError::Config(format!("LLM Token 环境变量 {name} 包含无法展开的引用")))
}

fn resolve_value(
    name: &str,
    locations: &[RegistryLocation],
    references_left: &mut usize,
) -> Result<Option<Zeroizing<String>>, ()> {
    let wide_name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let Some((value, expandable)) = locations
        .iter()
        .find_map(|&(root, subkey)| read_value(root, subkey, &wide_name))
    else {
        return Ok(None);
    };
    if !expandable {
        return Ok(Some(value));
    }

    let mut expanded = Zeroizing::new(String::new());
    let mut remainder = value.as_str();
    while let Some((prefix, candidate)) = remainder.split_once('%') {
        let Some((reference, tail)) = candidate.split_once('%') else {
            break;
        };
        expanded.push_str(prefix);
        if reference.is_empty() {
            expanded.push('%');
            remainder = candidate;
            continue;
        }
        *references_left = references_left.checked_sub(1).ok_or(())?;
        if reference.contains('=') {
            return Err(());
        }
        let replacement = match env::var(reference) {
            Ok(value) => Zeroizing::new(value),
            Err(env::VarError::NotPresent) => {
                resolve_value(reference, locations, references_left)?.ok_or(())?
            }
            Err(env::VarError::NotUnicode(_)) => return Err(()),
        };
        expanded.push_str(&replacement);
        if expanded.len() >= MAX_VALUE_UNITS * 3 {
            return Err(());
        }
        remainder = tail;
    }
    expanded.push_str(remainder);
    if expanded.encode_utf16().count() >= MAX_VALUE_UNITS {
        return Err(());
    }
    Ok(Some(expanded))
}

fn read_value(root: HKEY, subkey: PCWSTR, name: &[u16]) -> Option<(Zeroizing<String>, bool)> {
    let mut value = Zeroizing::new(vec![0u16; 256]);
    loop {
        let mut bytes = size_of_val(value.as_slice()) as u32;
        let mut value_type = 0;
        // SAFETY: The names are NUL-terminated, or subkey is null to query root directly.
        // bytes is initialized to the writable buffer's size.
        let result = unsafe {
            RegGetValueW(
                root,
                subkey,
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND | RRF_ZEROONFAILURE,
                &mut value_type,
                value.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        let units = bytes as usize / size_of::<u16>();
        match result {
            ERROR_SUCCESS => {
                let value = value.get(..units)?;
                let end = value.iter().position(|&unit| unit == 0)?;
                let text = Zeroizing::new(String::from_utf16(&value[..end]).ok()?);
                return Some((text, value_type == REG_EXPAND_SZ));
            }
            ERROR_MORE_DATA if units > value.len() && units <= MAX_VALUE_UNITS => {
                value.resize(units, 0);
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::secret::resolve_env_token;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegSetKeyValueW, KEY_QUERY_VALUE,
        KEY_SET_VALUE, REG_CREATED_NEW_KEY, REG_OPTION_VOLATILE, REG_SZ,
    };

    struct TempRegistryKey {
        path: Vec<u16>,
        handle: HKEY,
    }

    impl TempRegistryKey {
        fn new() -> Self {
            let path = format!(r"Software\L4A-Test-{}", uuid::Uuid::new_v4().simple());
            let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let mut handle = null_mut();
            let mut disposition = 0;
            // SAFETY: The path is NUL-terminated and both out parameters are writable.
            let result = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    null(),
                    REG_OPTION_VOLATILE,
                    KEY_QUERY_VALUE | KEY_SET_VALUE,
                    null(),
                    &mut handle,
                    &mut disposition,
                )
            };
            assert_eq!(result, ERROR_SUCCESS);
            assert_eq!(disposition, REG_CREATED_NEW_KEY);
            Self { path, handle }
        }

        fn set(&self, name: &str, value: &str, value_type: u32) {
            let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
            // SAFETY: The names and value are NUL-terminated and the byte count includes the NUL.
            let result = unsafe {
                RegSetKeyValueW(
                    self.handle,
                    null(),
                    name.as_ptr(),
                    value_type,
                    value.as_ptr().cast(),
                    size_of_val(value.as_slice()) as u32,
                )
            };
            assert_eq!(result, ERROR_SUCCESS);
        }
    }

    impl Drop for TempRegistryKey {
        fn drop(&mut self) {
            // SAFETY: This handle was opened in new, and path is still NUL-terminated.
            unsafe {
                RegCloseKey(self.handle);
                RegDeleteKeyW(HKEY_CURRENT_USER, self.path.as_ptr());
            }
        }
    }

    #[test]
    fn resolves_persisted_tokens_with_isolated_registry_keys() {
        let user = TempRegistryKey::new();
        let machine = TempRegistryKey::new();
        let locations = [(user.handle, null()), (machine.handle, null())];
        let name = format!("L4A_TEST_TOKEN_{}", uuid::Uuid::new_v4().simple());
        let reference = format!("{name}_SOURCE");
        let lookup = || resolve_env_token(&name, |name| read_from(name, &locations));

        assert!(lookup().is_err());
        machine.set(&name, " system-token ", REG_SZ);
        assert_eq!(lookup().unwrap().as_str(), "system-token");
        user.set(&name, " literal%token% ", REG_SZ);
        assert_eq!(lookup().unwrap().as_str(), "literal%token%");

        env::set_var(&name, "process-token");
        let token = lookup();
        env::remove_var(&name);
        assert_eq!(token.unwrap().as_str(), "process-token");

        user.set(&name, &format!("%{reference}%"), REG_EXPAND_SZ);
        assert!(matches!(lookup(), Err(CoreError::Config(err)) if err.contains("无法展开")));
        let long_token = "expanded-token".repeat(32);
        machine.set(&reference, &long_token, REG_SZ);
        assert_eq!(lookup().unwrap().as_str(), long_token);

        user.set(&reference, &format!("%{name}%"), REG_EXPAND_SZ);
        assert!(matches!(lookup(), Err(CoreError::Config(err)) if err.contains("无法展开")));
        user.set(&name, " \t ", REG_SZ);
        assert!(matches!(lookup(), Err(CoreError::Config(err)) if err.contains("值为空")));
    }
}
